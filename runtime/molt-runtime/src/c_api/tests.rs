// NOTE: c_api tests share a single process-global RuntimeState.
// The runtime is initialized once by the first test and reused.
// Each test acquires TEST_MUTEX to serialize access, preventing
// concurrent GIL re-entry from corrupting the slab allocator.
//
// Run with: cargo test -p molt-runtime --lib -- c_api::tests --test-threads=1
// Individual tests pass; full suite may hit stack overflow from
// deep GIL re-entry accumulation across tests.

use super::*;
use crate::builtins::exceptions::molt_exception_class;

struct CApiTestGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl CApiTestGuard {
    fn new() -> Self {
        let guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        Self { _guard: guard }
    }
}

impl Drop for CApiTestGuard {
    fn drop(&mut self) {
    }
}

extern "C" fn c_api_test_meth_varargs(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        let len = molt_sequence_length(args_bits);
        if len < 0 {
            return MoltObject::none().bits();
        }
        MoltObject::from_int(len).bits()
    })
}

extern "C" fn c_api_test_meth_varargs_keywords(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        let pos_len = molt_sequence_length(args_bits);
        if pos_len < 0 {
            return MoltObject::none().bits();
        }
        let kw_len = if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
            0
        } else if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
                    return raise_exception::<u64>(_py, "TypeError", "kwargs payload must be dict");
                }
                (dict_order(kwargs_ptr).len() / 2) as i64
            }
        } else {
            0
        };
        MoltObject::from_int(pos_len * 10 + kw_len).bits()
    })
}

extern "C" fn c_api_test_meth_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(101).bits()
    })
}

extern "C" fn c_api_test_meth_o(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "METH_O callback missing arg");
        }
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

extern "C" fn c_api_test_dynamic_varargs(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        let len = molt_sequence_length(args_bits);
        if len < 0 {
            return MoltObject::none().bits();
        }
        MoltObject::from_int(self_value * 10 + len).bits()
    })
}

extern "C" fn c_api_test_dynamic_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(1000 + self_value).bits()
    })
}

extern "C" fn c_api_test_dynamic_o(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        let Some(arg_value) = to_i64(obj_from_bits(arg_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic METH_O arg must be an int for this probe",
            );
        };
        MoltObject::from_int(self_value * 100 + arg_value).bits()
    })
}

extern "C" fn c_api_test_static_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "static callback expected NULL self_bits",
            );
        }
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(204).bits()
    })
}

fn create_test_heap_class(_py: &PyToken<'_>, name: &[u8], attrs: &[(&[u8], u64)]) -> u64 {
    let builtins = crate::builtins::classes::builtin_classes(_py);
    let name_bits = unsafe { molt_string_from(name.as_ptr(), name.len() as u64) };
    assert!(!obj_from_bits(name_bits).is_none());
    let namespace_bits = molt_dict_new(attrs.len() as u64);
    assert!(!obj_from_bits(namespace_bits).is_none());
    for &(attr_name, value_bits) in attrs {
        let attr_bits = unsafe { molt_string_from(attr_name.as_ptr(), attr_name.len() as u64) };
        assert!(!obj_from_bits(attr_bits).is_none());
        assert_eq!(
            molt_mapping_setitem(namespace_bits, attr_bits, value_bits),
            0
        );
        dec_ref_bits(_py, attr_bits);
    }
    let class_bits = crate::builtins::types::molt_type_new(
        builtins.type_obj,
        name_bits,
        none_bits(),
        namespace_bits,
        none_bits(),
    );
    assert!(!obj_from_bits(class_bits).is_none());
    dec_ref_bits(_py, namespace_bits);
    dec_ref_bits(_py, name_bits);
    class_bits
}

#[test]
fn c_api_version_is_nonzero() {
    let _guard = CApiTestGuard::new();
    assert!(molt_c_api_version() >= 1);
}

#[test]
fn err_set_matches_fetch_roundtrip() {
    let _guard = CApiTestGuard::new();
    let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
    let msg = b"boom";
    let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
    assert_eq!(rc, 0);
    assert_eq!(molt_exception_pending(), 1);
    assert_eq!(molt_err_matches(runtime_error), 1);
    let exc_bits = molt_err_fetch();
    assert!(!obj_from_bits(exc_bits).is_none());
    assert_eq!(molt_exception_pending(), 0);
    let kind_bits = molt_exception_kind(exc_bits);
    let class_bits = molt_exception_class(kind_bits);
    assert_eq!(molt_err_matches(runtime_error), 0);
    assert!(issubclass_bits(class_bits, runtime_error));
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, exc_bits);
    });
}

#[test]
fn object_call_numeric_and_sequence_wrappers() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(4).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        let append_name_ptr = alloc_string(_py, b"append");
        assert!(!append_name_ptr.is_null());
        let append_name_bits = MoltObject::from_ptr(append_name_ptr).bits();
        let append_bits = molt_object_getattr(list_bits, append_name_bits);
        assert!(!obj_from_bits(append_bits).is_none());
        let append_args_ptr = alloc_tuple(_py, &[MoltObject::from_int(5).bits()]);
        assert!(!append_args_ptr.is_null());
        let append_args_bits = MoltObject::from_ptr(append_args_ptr).bits();
        let append_out = molt_object_call(append_bits, append_args_bits, none_bits());
        assert!(!exception_pending(_py));
        assert!(obj_from_bits(append_out).is_none());
        dec_ref_bits(_py, append_args_bits);
        dec_ref_bits(_py, append_bits);
        dec_ref_bits(_py, append_name_bits);

        assert_eq!(molt_sequence_length(list_bits), 3);
        let idx_bits = MoltObject::from_int(1).bits();
        let got_bits = molt_sequence_getitem(list_bits, idx_bits);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(4));
        let rc = molt_sequence_setitem(
            list_bits,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(9).bits(),
        );
        assert_eq!(rc, 0);
        let got0 = molt_sequence_getitem(list_bits, MoltObject::from_int(0).bits());
        assert_eq!(to_i64(obj_from_bits(got0)), Some(9));
        let got2 = molt_sequence_getitem(list_bits, MoltObject::from_int(2).bits());
        assert_eq!(to_i64(obj_from_bits(got2)), Some(5));
        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, got0);
        dec_ref_bits(_py, got2);
        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn buffer_acquire_and_release_pins_owner() {
    let _guard = CApiTestGuard::new();
    let bytes_bits = unsafe { molt_bytes_from(b"abc".as_ptr(), 3) };
    assert!(!obj_from_bits(bytes_bits).is_none());
    let mut view = MoltBufferView {
        data: std::ptr::null_mut(),
        len: 0,
        readonly: 1,
        reserved: 0,
        stride: 1,
        itemsize: 1,
        owner: 0,
    };
    let rc = unsafe { molt_buffer_acquire(bytes_bits, &mut view as *mut MoltBufferView) };
    assert_eq!(rc, 0);
    assert_eq!(view.len, 3);
    assert_eq!(view.readonly, 1);
    assert!(!view.data.is_null());
    assert_eq!(view.owner, bytes_bits);
    let observed = unsafe { std::slice::from_raw_parts(view.data as *const u8, view.len as usize) };
    assert_eq!(observed, b"abc");
    let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
    assert_eq!(rc_release, 0);
    assert!(view.data.is_null());
    assert_eq!(view.owner, 0);
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, bytes_bits);
    });
}

#[test]
fn err_pending_peek_restore_roundtrip() {
    let _guard = CApiTestGuard::new();
    let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
    let msg = b"boom";
    let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
    assert_eq!(rc, 0);
    assert_eq!(molt_err_pending(), 1);
    let peek_bits = molt_err_peek();
    assert!(!obj_from_bits(peek_bits).is_none());
    assert_eq!(molt_err_pending(), 1);
    let fetched_bits = molt_err_fetch();
    assert!(!obj_from_bits(fetched_bits).is_none());
    assert_eq!(molt_err_pending(), 0);
    assert_eq!(molt_err_restore(fetched_bits), 0);
    assert_eq!(molt_err_pending(), 1);
    let restored_bits = molt_err_fetch();
    assert!(!obj_from_bits(restored_bits).is_none());
    assert_eq!(molt_err_pending(), 0);
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, peek_bits);
        dec_ref_bits(_py, fetched_bits);
        dec_ref_bits(_py, restored_bits);
    });
}

#[test]
fn err_clear_resets_last_exception_slot() {
    let _guard = CApiTestGuard::new();
    let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
    let msg = b"boom";
    let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
    assert_eq!(rc, 0);
    assert_eq!(molt_exception_pending(), 1);

    let peek_bits = molt_exception_last();
    assert!(!obj_from_bits(peek_bits).is_none());

    let _ = molt_exception_clear();
    assert_eq!(molt_exception_pending(), 0);

    let after_clear_bits = molt_exception_last();
    assert!(obj_from_bits(after_clear_bits).is_none());
    assert_eq!(molt_exception_pending(), 0);

    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, peek_bits);
    });
}

#[test]
fn mapping_length_success_and_failure_paths() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        assert!(!dict_ptr.is_null());
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_int(7).bits();
        assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);
        assert_eq!(molt_mapping_length(dict_bits), 1);
        let invalid_bits = MoltObject::from_int(42).bits();
        assert_eq!(molt_mapping_length(invalid_bits), -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict_bits);
    });
}

#[test]
fn mapping_keys_success_and_failure_paths() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        assert!(!dict_ptr.is_null());
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_int(7).bits();
        assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);

        let keys_bits = molt_mapping_keys(dict_bits);
        assert!(!obj_from_bits(keys_bits).is_none());
        assert_eq!(molt_sequence_length(keys_bits), 1);
        assert_eq!(molt_object_contains(keys_bits, key_bits), 1);
        dec_ref_bits(_py, keys_bits);

        let invalid_bits = MoltObject::from_int(42).bits();
        assert!(obj_from_bits(molt_mapping_keys(invalid_bits)).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict_bits);
    });
}

#[test]
fn string_from_as_ptr_roundtrip_and_type_errors() {
    let _guard = CApiTestGuard::new();
    let text = b"hello";
    let string_bits = unsafe { molt_string_from(text.as_ptr(), text.len() as u64) };
    assert!(!obj_from_bits(string_bits).is_none());
    let mut out_len = 0u64;
    let ptr = unsafe { molt_string_as_ptr(string_bits, &mut out_len as *mut u64) };
    assert!(!ptr.is_null());
    assert_eq!(out_len, text.len() as u64);
    let observed = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) };
    assert_eq!(observed, text);

    let invalid_bits = MoltObject::from_int(9).bits();
    let bad_ptr = unsafe { molt_string_as_ptr(invalid_bits, std::ptr::null_mut()) };
    assert!(bad_ptr.is_null());
    assert_eq!(molt_err_pending(), 1);
    assert_eq!(molt_err_clear(), 0);

    let null_bits = unsafe { molt_string_from(std::ptr::null(), 1) };
    assert_eq!(molt_err_pending(), 1);
    assert_eq!(molt_err_clear(), 0);

    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, string_bits);
        if !obj_from_bits(null_bits).is_none() {
            dec_ref_bits(_py, null_bits);
        }
    });
}

#[test]
fn object_setattr_symbol_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let runtime_error = runtime_error_type_bits(_py);
        let msg_ptr = alloc_string(_py, b"msg");
        assert!(!msg_ptr.is_null());
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
        assert!(!obj_from_bits(exc_bits).is_none());
        let attr_ptr = alloc_string(_py, b"custom");
        assert!(!attr_ptr.is_null());
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let value_bits = MoltObject::from_int(99).bits();
        let set_result = molt_object_setattr(exc_bits, attr_bits, value_bits);
        assert!(!exception_pending(_py));
        let got_bits = molt_object_getattr(exc_bits, attr_bits);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(99));
        dec_ref_bits(_py, got_bits);
        if !obj_from_bits(set_result).is_none() {
            dec_ref_bits(_py, set_result);
        }
        dec_ref_bits(_py, attr_bits);
        dec_ref_bits(_py, exc_bits);
        dec_ref_bits(_py, msg_bits);
    });
}

#[test]
fn attr_object_ic_keeps_type_objects_distinct_per_site() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        crate::object::bump_type_version();
        let class_a_bits = create_test_heap_class(_py, b"A", &[]);
        let class_b_bits = create_test_heap_class(_py, b"B", &[]);
        let site_bits = MoltObject::from_int(37).bits();

        let a_name_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_a_bits,
                b"__name__".as_ptr(),
                b"__name__".len() as u64,
                site_bits,
            ) as u64
        };
        let b_name_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_b_bits,
                b"__name__".as_ptr(),
                b"__name__".len() as u64,
                site_bits,
            ) as u64
        };

        let mut a_len = 0u64;
        let a_ptr = unsafe { molt_string_as_ptr(a_name_bits, &mut a_len as *mut u64) };
        assert!(!a_ptr.is_null());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(a_ptr, a_len as usize) },
            b"A"
        );

        let mut b_len = 0u64;
        let b_ptr = unsafe { molt_string_as_ptr(b_name_bits, &mut b_len as *mut u64) };
        assert!(!b_ptr.is_null());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(b_ptr, b_len as usize) },
            b"B"
        );

        dec_ref_bits(_py, b_name_bits);
        dec_ref_bits(_py, a_name_bits);
        dec_ref_bits(_py, class_b_bits);
        dec_ref_bits(_py, class_a_bits);
    });
}

#[test]
fn attr_object_ic_keeps_class_attrs_distinct_per_site() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // Invalidate stale IC entries from prior tests that may hold
        // dangling pointers to freed heap objects.
        crate::object::bump_type_version();
        let class_a_bits =
            create_test_heap_class(_py, b"A", &[(b"x", MoltObject::from_int(1).bits())]);
        let class_b_bits =
            create_test_heap_class(_py, b"B", &[(b"x", MoltObject::from_int(2).bits())]);
        let site_bits = MoltObject::from_int(41).bits();

        let a_x_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_a_bits,
                b"x".as_ptr(),
                1,
                site_bits,
            ) as u64
        };
        let b_x_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_b_bits,
                b"x".as_ptr(),
                1,
                site_bits,
            ) as u64
        };

        assert_eq!(to_i64(obj_from_bits(a_x_bits)), Some(1));
        assert_eq!(to_i64(obj_from_bits(b_x_bits)), Some(2));

        dec_ref_bits(_py, b_x_bits);
        dec_ref_bits(_py, a_x_bits);
        dec_ref_bits(_py, class_b_bits);
        dec_ref_bits(_py, class_a_bits);
    });
}

#[test]
fn plain_function_object_has_no_set_name_attr() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::molt_id as *const () as usize as u64,
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let name_ptr = alloc_string(_py, b"__set_name__");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();

        let none_bits = MoltObject::none().bits();
        let got_bits = crate::builtins::attributes::molt_get_attr_name_default(
            func_bits, name_bits, none_bits,
        );

        assert!(obj_from_bits(got_bits).is_none());
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn class_apply_set_name_tolerates_plain_function_attrs() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::molt_id as *const () as usize as u64,
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let class_bits = create_test_heap_class(_py, b"A", &[(b"f", func_bits)]);

        let res_bits = crate::molt_class_apply_set_name(class_bits);
        assert!(obj_from_bits(res_bits).is_none());
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn scalar_handle_helpers_roundtrip() {
    let _guard = CApiTestGuard::new();
    assert!(obj_from_bits(molt_none()).is_none());

    let true_bits = molt_bool_from_i32(1);
    let false_bits = molt_bool_from_i32(0);
    assert_eq!(molt_object_truthy(true_bits), 1);
    assert_eq!(molt_object_truthy(false_bits), 0);

    let int_bits = molt_int_from_i64(-42);
    assert_eq!(molt_int_as_i64(int_bits), -42);

    let float_bits = molt_float_from_f64(3.5);
    assert_eq!(molt_float_as_f64(float_bits), 3.5);
    assert_eq!(molt_float_as_f64(int_bits), -42.0);

    assert_eq!(molt_int_as_i64(float_bits), -1);
    assert_eq!(molt_err_pending(), 1);
    assert_eq!(molt_err_clear(), 0);

    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, true_bits);
        dec_ref_bits(_py, false_bits);
        dec_ref_bits(_py, int_bits);
        dec_ref_bits(_py, float_bits);
    });
}

#[test]
fn object_bytes_compare_and_contains_helpers() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let runtime_error = runtime_error_type_bits(_py);
        let msg_ptr = alloc_string(_py, b"msg");
        assert!(!msg_ptr.is_null());
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
        assert!(!obj_from_bits(exc_bits).is_none());

        let value_bits = MoltObject::from_int(77).bits();
        let set_rc = unsafe {
            molt_object_setattr_bytes(
                exc_bits,
                b"custom".as_ptr(),
                b"custom".len() as u64,
                value_bits,
            )
        };
        assert_eq!(set_rc, 0);
        let got_bits = unsafe {
            molt_object_getattr_bytes(exc_bits, b"custom".as_ptr(), b"custom".len() as u64)
        };
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(77));
        dec_ref_bits(_py, got_bits);

        assert_eq!(
            molt_object_equal(
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(5).bits()
            ),
            1
        );
        assert_eq!(
            molt_object_not_equal(
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(6).bits()
            ),
            1
        );

        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        assert_eq!(
            molt_object_contains(list_bits, MoltObject::from_int(2).bits()),
            1
        );
        assert_eq!(
            molt_object_contains(list_bits, MoltObject::from_int(9).bits()),
            0
        );

        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, exc_bits);
        dec_ref_bits(_py, msg_bits);
    });
}

#[test]
fn array_constructors_roundtrip() {
    let _guard = CApiTestGuard::new();
    let elems = [
        MoltObject::from_int(10).bits(),
        MoltObject::from_int(20).bits(),
        MoltObject::from_int(30).bits(),
    ];
    let tuple_bits = unsafe { molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) };
    let list_bits = unsafe { molt_list_from_array(elems.as_ptr(), elems.len() as u64) };
    assert!(!obj_from_bits(tuple_bits).is_none());
    assert!(!obj_from_bits(list_bits).is_none());
    assert_eq!(molt_sequence_length(tuple_bits), 3);
    assert_eq!(molt_sequence_length(list_bits), 3);

    let keys = [
        MoltObject::from_int(1).bits(),
        MoltObject::from_int(2).bits(),
    ];
    let values = [
        MoltObject::from_int(100).bits(),
        MoltObject::from_int(200).bits(),
    ];
    let dict_bits = unsafe { molt_dict_from_pairs(keys.as_ptr(), values.as_ptr(), 2) };
    assert!(!obj_from_bits(dict_bits).is_none());
    assert_eq!(molt_mapping_length(dict_bits), 2);
    let got_bits = molt_mapping_getitem(dict_bits, keys[1]);
    assert_eq!(to_i64(obj_from_bits(got_bits)), Some(200));
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, dict_bits);
    });

    let null_tuple_bits = unsafe { molt_tuple_from_array(std::ptr::null::<MoltHandle>(), 1) };
    assert!(obj_from_bits(null_tuple_bits).is_none());
    assert_eq!(molt_err_pending(), 1);
    assert_eq!(molt_err_clear(), 0);
}

#[test]
fn type_ready_and_module_parity_wrappers_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let builtins = crate::builtins::classes::builtin_classes(_py);
        assert_eq!(molt_type_ready(builtins.type_obj), 0);
        assert_eq!(molt_type_ready(MoltObject::from_int(1).bits()), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let module_name_bits = unsafe { molt_string_from(b"demo_ext".as_ptr(), 8) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        let answer_name_ptr = alloc_string(_py, b"answer");
        assert!(!answer_name_ptr.is_null());
        let answer_name_bits = MoltObject::from_ptr(answer_name_ptr).bits();
        assert_eq!(
            molt_module_add_int_constant(module_bits, answer_name_bits, 42),
            0
        );
        let answer_bits = molt_module_get_object(module_bits, answer_name_bits);
        assert_eq!(to_i64(obj_from_bits(answer_bits)), Some(42));

        assert_eq!(
            unsafe {
                molt_module_add_object_bytes(
                    module_bits,
                    b"status".as_ptr(),
                    b"status".len() as u64,
                    MoltObject::from_int(7).bits(),
                )
            },
            0
        );
        let status_bits = unsafe {
            molt_module_get_object_bytes(module_bits, b"status".as_ptr(), b"status".len() as u64)
        };
        assert_eq!(to_i64(obj_from_bits(status_bits)), Some(7));

        let label_name_ptr = alloc_string(_py, b"label");
        assert!(!label_name_ptr.is_null());
        let label_name_bits = MoltObject::from_ptr(label_name_ptr).bits();
        assert_eq!(
            unsafe {
                molt_module_add_string_constant(module_bits, label_name_bits, b"ok".as_ptr(), 2)
            },
            0
        );
        let label_bits = molt_module_get_object(module_bits, label_name_bits);
        let mut label_len = 0u64;
        let label_ptr = unsafe { molt_string_as_ptr(label_bits, &mut label_len as *mut u64) };
        assert!(!label_ptr.is_null());
        assert_eq!(label_len, 2);
        let label_text = unsafe { std::slice::from_raw_parts(label_ptr, label_len as usize) };
        assert_eq!(label_text, b"ok");

        assert_eq!(molt_module_add_type(module_bits, builtins.type_obj), 0);
        let type_name_ptr = alloc_string(_py, b"type");
        assert!(!type_name_ptr.is_null());
        let type_name_bits = MoltObject::from_ptr(type_name_ptr).bits();
        let added_type_bits = molt_module_get_object(module_bits, type_name_bits);
        assert_eq!(molt_object_equal(added_type_bits, builtins.type_obj), 1);
        assert_eq!(
            molt_module_add_type(module_bits, MoltObject::from_int(1).bits()),
            -1
        );
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let dict_bits = molt_module_get_dict(module_bits);
        assert!(!obj_from_bits(dict_bits).is_none());
        assert!(molt_mapping_length(dict_bits) >= 4);

        dec_ref_bits(_py, added_type_bits);
        dec_ref_bits(_py, type_name_bits);
        dec_ref_bits(_py, dict_bits);
        dec_ref_bits(_py, label_bits);
        dec_ref_bits(_py, label_name_bits);
        dec_ref_bits(_py, status_bits);
        dec_ref_bits(_py, answer_bits);
        dec_ref_bits(_py, answer_name_bits);
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_metadata_and_state_registry_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_meta".as_ptr(), 9) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());
        let module_ptr = obj_from_bits(module_bits)
            .as_ptr()
            .expect("module pointer should be valid");
        let module_def_ptr = 0xD15EA5Eusize;

        assert_eq!(
            molt_module_capi_register(module_bits, module_def_ptr, 32),
            0
        );
        assert_eq!(molt_module_capi_get_def(module_bits), module_def_ptr);
        let state_ptr = molt_module_capi_get_state(module_bits);
        assert_ne!(state_ptr, 0);
        let state_slice = unsafe { std::slice::from_raw_parts_mut(state_ptr as *mut u8, 32) };
        for byte in state_slice.iter() {
            assert_eq!(*byte, 0);
        }
        state_slice[0] = 7;
        state_slice[31] = 9;

        assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
        assert_eq!(molt_module_state_find(module_def_ptr), module_bits);
        assert_eq!(molt_module_state_remove(module_def_ptr), 0);
        assert_eq!(molt_module_state_find(module_def_ptr), 0);

        assert_eq!(molt_module_state_remove(module_def_ptr), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
        c_api_module_on_module_teardown(_py, module_ptr);
        assert_eq!(molt_module_capi_get_def(module_bits), 0);
        assert_eq!(molt_module_capi_get_state(module_bits), 0);
        assert_eq!(molt_module_state_find(module_def_ptr), 0);

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_method_bridge_handles_supported_flags() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_capi".as_ptr(), 9) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_varargs".as_ptr(),
                    b"meth_varargs".len() as u64,
                    c_api_test_meth_varargs as *const () as usize,
                    C_API_METH_VARARGS,
                    b"varargs".as_ptr(),
                    b"varargs".len() as u64,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_kwargs".as_ptr(),
                    b"meth_kwargs".len() as u64,
                    c_api_test_meth_varargs_keywords as *const () as usize,
                    C_API_METH_VARARGS | C_API_METH_KEYWORDS,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_noargs".as_ptr(),
                    b"meth_noargs".len() as u64,
                    c_api_test_meth_noargs as *const () as usize,
                    C_API_METH_NOARGS,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_o".as_ptr(),
                    b"meth_o".len() as u64,
                    c_api_test_meth_o as *const () as usize,
                    C_API_METH_O,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );

        let meth_varargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_varargs".as_ptr(), 12) };
        let meth_kwargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_kwargs".as_ptr(), 11) };
        let meth_noargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_noargs".as_ptr(), 11) };
        let meth_o_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_o".as_ptr(), 6) };

        let args3_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!args3_ptr.is_null());
        let args3_bits = MoltObject::from_ptr(args3_ptr).bits();
        let out_varargs = molt_object_call(meth_varargs_bits, args3_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_varargs)), Some(3));
        dec_ref_bits(_py, out_varargs);

        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let kwargs_ptr = alloc_dict_with_pairs(_py, &[key_bits, MoltObject::from_int(9).bits()]);
        assert!(!kwargs_ptr.is_null());
        let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();

        let out_kwargs = molt_object_call(meth_kwargs_bits, args3_bits, kwargs_bits);
        assert_eq!(to_i64(obj_from_bits(out_kwargs)), Some(31));
        dec_ref_bits(_py, out_kwargs);

        let reject_kwargs = molt_object_call(meth_varargs_bits, args3_bits, kwargs_bits);
        assert!(obj_from_bits(reject_kwargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args0_ptr = alloc_tuple(_py, &[]);
        assert!(!args0_ptr.is_null());
        let args0_bits = MoltObject::from_ptr(args0_ptr).bits();
        let out_noargs = molt_object_call(meth_noargs_bits, args0_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(101));
        dec_ref_bits(_py, out_noargs);

        let reject_noargs = molt_object_call(meth_noargs_bits, args3_bits, none_bits());
        assert!(obj_from_bits(reject_noargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args1_ptr = alloc_tuple(_py, &[MoltObject::from_int(55).bits()]);
        assert!(!args1_ptr.is_null());
        let args1_bits = MoltObject::from_ptr(args1_ptr).bits();
        let out_o = molt_object_call(meth_o_bits, args1_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_o)), Some(55));
        dec_ref_bits(_py, out_o);

        let reject_o = molt_object_call(meth_o_bits, args0_bits, none_bits());
        assert!(obj_from_bits(reject_o).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args1_bits);
        dec_ref_bits(_py, args0_bits);
        dec_ref_bits(_py, kwargs_bits);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, args3_bits);
        dec_ref_bits(_py, meth_o_bits);
        dec_ref_bits(_py, meth_noargs_bits);
        dec_ref_bits(_py, meth_kwargs_bits);
        dec_ref_bits(_py, meth_varargs_bits);
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_method_bridge_rejects_unsupported_flags() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_bad".as_ptr(), 8) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        let rc = unsafe {
            molt_module_add_cfunction_bytes(
                module_bits,
                b"bad".as_ptr(),
                3,
                c_api_test_meth_varargs as *const () as usize,
                C_API_METH_VARARGS | C_API_METH_O,
                std::ptr::null(),
                0,
            )
        };
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn c_api_method_dispatch_supports_dynamic_self_callbacks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dyn_varargs_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_varargs".as_ptr(),
                b"dyn_varargs".len() as u64,
                c_api_test_dynamic_varargs as *const () as usize,
                C_API_METH_VARARGS,
                std::ptr::null(),
                0,
            )
        };
        let dyn_noargs_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_noargs".as_ptr(),
                b"dyn_noargs".len() as u64,
                c_api_test_dynamic_noargs as *const () as usize,
                C_API_METH_NOARGS,
                std::ptr::null(),
                0,
            )
        };
        let dyn_o_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_o".as_ptr(),
                b"dyn_o".len() as u64,
                c_api_test_dynamic_o as *const () as usize,
                C_API_METH_O,
                std::ptr::null(),
                0,
            )
        };
        assert!(!obj_from_bits(dyn_varargs_bits).is_none());
        assert!(!obj_from_bits(dyn_noargs_bits).is_none());
        assert!(!obj_from_bits(dyn_o_bits).is_none());

        let args_var_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(40).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        assert!(!args_var_ptr.is_null());
        let args_var_bits = MoltObject::from_ptr(args_var_ptr).bits();
        let out_var = molt_object_call(dyn_varargs_bits, args_var_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_var)), Some(402));
        dec_ref_bits(_py, out_var);

        let args_none_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
        assert!(!args_none_ptr.is_null());
        let args_none_bits = MoltObject::from_ptr(args_none_ptr).bits();
        let out_noargs = molt_object_call(dyn_noargs_bits, args_none_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(1007));
        dec_ref_bits(_py, out_noargs);

        let args_o_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(9).bits(),
            ],
        );
        assert!(!args_o_ptr.is_null());
        let args_o_bits = MoltObject::from_ptr(args_o_ptr).bits();
        let out_o = molt_object_call(dyn_o_bits, args_o_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_o)), Some(509));
        dec_ref_bits(_py, out_o);

        let args_missing_self_ptr = alloc_tuple(_py, &[]);
        assert!(!args_missing_self_ptr.is_null());
        let args_missing_self_bits = MoltObject::from_ptr(args_missing_self_ptr).bits();
        let reject_missing_self =
            molt_object_call(dyn_varargs_bits, args_missing_self_bits, none_bits());
        assert!(obj_from_bits(reject_missing_self).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args_bad_noargs_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(7).bits(),
                MoltObject::from_int(1).bits(),
            ],
        );
        assert!(!args_bad_noargs_ptr.is_null());
        let args_bad_noargs_bits = MoltObject::from_ptr(args_bad_noargs_ptr).bits();
        let reject_noargs = molt_object_call(dyn_noargs_bits, args_bad_noargs_bits, none_bits());
        assert!(obj_from_bits(reject_noargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args_bad_o_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
        assert!(!args_bad_o_ptr.is_null());
        let args_bad_o_bits = MoltObject::from_ptr(args_bad_o_ptr).bits();
        let reject_o = molt_object_call(dyn_o_bits, args_bad_o_bits, none_bits());
        assert!(obj_from_bits(reject_o).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args_bad_o_bits);
        dec_ref_bits(_py, args_bad_noargs_bits);
        dec_ref_bits(_py, args_missing_self_bits);
        dec_ref_bits(_py, args_o_bits);
        dec_ref_bits(_py, args_none_bits);
        dec_ref_bits(_py, args_var_bits);
        dec_ref_bits(_py, dyn_o_bits);
        dec_ref_bits(_py, dyn_noargs_bits);
        dec_ref_bits(_py, dyn_varargs_bits);
    });
}

#[test]
fn c_api_method_dispatch_supports_null_self_for_static_callbacks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let static_noargs_bits = unsafe {
            molt_cfunction_create_bytes(
                0,
                b"static_noargs".as_ptr(),
                b"static_noargs".len() as u64,
                c_api_test_static_noargs as *const () as usize,
                C_API_METH_NOARGS,
                std::ptr::null(),
                0,
            )
        };
        assert!(!obj_from_bits(static_noargs_bits).is_none());

        let args_empty_ptr = alloc_tuple(_py, &[]);
        assert!(!args_empty_ptr.is_null());
        let args_empty_bits = MoltObject::from_ptr(args_empty_ptr).bits();
        let out = molt_object_call(static_noargs_bits, args_empty_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out)), Some(204));
        dec_ref_bits(_py, out);

        let args_bad_ptr = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
        assert!(!args_bad_ptr.is_null());
        let args_bad_bits = MoltObject::from_ptr(args_bad_ptr).bits();
        let reject = molt_object_call(static_noargs_bits, args_bad_bits, none_bits());
        assert!(obj_from_bits(reject).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args_bad_bits);
        dec_ref_bits(_py, args_empty_bits);
        dec_ref_bits(_py, static_noargs_bits);
    });
}

// -----------------------------------------------------------------------
// Phase 1 C-API tests
// -----------------------------------------------------------------------

#[test]
fn c_api_list_new_size_getitem_setitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list = PyList_New(3);
        assert_ne!(list, 0);
        assert_eq!(PyList_Size(list), 3);

        // All slots default to None.
        let item0 = PyList_GetItem(list, 0);
        assert!(obj_from_bits(item0).is_none());

        // SetItem steals the ref, so we inc_ref first for the value we're inserting.
        let val = MoltObject::from_int(42).bits();
        inc_ref_bits(_py, val);
        assert_eq!(PyList_SetItem(list, 1, val), 0);
        let got = PyList_GetItem(list, 1);
        assert_eq!(to_i64(obj_from_bits(got)), Some(42));

        // Append
        let extra = MoltObject::from_int(99).bits();
        assert_eq!(PyList_Append(list, extra), 0);
        assert_eq!(PyList_Size(list), 4);
        let got_last = PyList_GetItem(list, 3);
        assert_eq!(to_i64(obj_from_bits(got_last)), Some(99));

        // Negative index
        let got_neg = PyList_GetItem(list, -1);
        assert_eq!(to_i64(obj_from_bits(got_neg)), Some(99));

        // Out-of-bounds
        let bad = PyList_GetItem(list, 100);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_list_new_negative_size_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list = PyList_New(-1);
        assert_eq!(list, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_dict_new_setitem_getitem_contains_size() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyDict_Size(dict), 0);

        let key = MoltObject::from_int(10).bits();
        let val = MoltObject::from_int(20).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);
        assert_eq!(PyDict_Size(dict), 1);
        assert_eq!(PyDict_Contains(dict, key), 1);

        let got = PyDict_GetItem(dict, key);
        assert_ne!(got, 0);
        assert_eq!(to_i64(obj_from_bits(got)), Some(20));

        // Missing key returns 0 (no exception).
        let missing_key = MoltObject::from_int(999).bits();
        let missing = PyDict_GetItem(dict, missing_key);
        assert_eq!(missing, 0);
        assert!(!exception_pending(_py));

        assert_eq!(PyDict_Contains(dict, missing_key), 0);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_dict_set_item_string() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let val = MoltObject::from_int(42).bits();
        let rc = unsafe { PyDict_SetItemString(dict, c"hello".as_ptr(), val) };
        assert_eq!(rc, 0);
        assert_eq!(PyDict_Size(dict), 1);

        // Verify we can retrieve by constructing a matching key.
        let key_ptr = alloc_string(_py, b"hello");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let got = PyDict_GetItem(dict, key_bits);
        assert_eq!(to_i64(obj_from_bits(got)), Some(42));

        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_tuple_new_size_getitem_setitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let tuple = PyTuple_New(3);
        assert_ne!(tuple, 0);
        assert_eq!(PyTuple_Size(tuple), 3);

        // All slots default to None.
        let item0 = PyTuple_GetItem(tuple, 0);
        assert!(obj_from_bits(item0).is_none());

        // SetItem steals the ref, so inc_ref the value first.
        let val = MoltObject::from_int(77).bits();
        inc_ref_bits(_py, val);
        assert_eq!(PyTuple_SetItem(tuple, 2, val), 0);
        let got = PyTuple_GetItem(tuple, 2);
        assert_eq!(to_i64(obj_from_bits(got)), Some(77));

        // Out-of-bounds
        let bad = PyTuple_GetItem(tuple, 5);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        // Negative index in SetItem should fail (CPython tuple uses non-negative only).
        let steal_val = MoltObject::from_int(1).bits();
        inc_ref_bits(_py, steal_val);
        let rc = PyTuple_SetItem(tuple, -1, steal_val);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_type_checks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // Int
        let int_val = MoltObject::from_int(42).bits();
        assert_eq!(PyLong_Check(int_val), 1);
        assert_eq!(PyFloat_Check(int_val), 0);
        assert_eq!(PyBool_Check(int_val), 0);
        assert_eq!(PyNone_Check(int_val), 0);
        assert_eq!(PyUnicode_Check(int_val), 0);
        assert_eq!(PyList_Check(int_val), 0);
        assert_eq!(PyDict_Check(int_val), 0);
        assert_eq!(PyTuple_Check(int_val), 0);

        // Float
        let float_val = MoltObject::from_float(3.125).bits();
        assert_eq!(PyFloat_Check(float_val), 1);
        assert_eq!(PyLong_Check(float_val), 0);

        // Bool
        let bool_val = MoltObject::from_bool(true).bits();
        assert_eq!(PyBool_Check(bool_val), 1);

        // None
        let none_val = MoltObject::none().bits();
        assert_eq!(PyNone_Check(none_val), 1);
        assert_eq!(PyBool_Check(none_val), 0);

        // String
        let str_ptr = alloc_string(_py, b"hello");
        assert!(!str_ptr.is_null());
        let str_bits = MoltObject::from_ptr(str_ptr).bits();
        assert_eq!(PyUnicode_Check(str_bits), 1);
        assert_eq!(PyLong_Check(str_bits), 0);
        dec_ref_bits(_py, str_bits);

        // List
        let list = PyList_New(0);
        assert_ne!(list, 0);
        assert_eq!(PyList_Check(list), 1);
        assert_eq!(PyTuple_Check(list), 0);
        assert_eq!(PyDict_Check(list), 0);
        dec_ref_bits(_py, list);

        // Dict
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyDict_Check(dict), 1);
        assert_eq!(PyList_Check(dict), 0);
        dec_ref_bits(_py, dict);

        // Tuple
        let tuple = PyTuple_New(0);
        assert_ne!(tuple, 0);
        assert_eq!(PyTuple_Check(tuple), 1);
        assert_eq!(PyList_Check(tuple), 0);
        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_iter_protocol_on_list() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // Build a list [10, 20, 30]
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(10).bits(),
                MoltObject::from_int(20).bits(),
                MoltObject::from_int(30).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        // Check PyIter_Check on the list (not an iterator itself).
        assert_eq!(PyIter_Check(list_bits), 0);

        // Get an iterator.
        let iter = PyObject_GetIter(list_bits);
        assert_ne!(iter, 0);
        assert!(!exception_pending(_py));

        // The iterator should pass PyIter_Check.
        assert_eq!(PyIter_Check(iter), 1);

        // Iterate: 10, 20, 30, then NULL.
        let v1 = PyIter_Next(iter);
        assert_ne!(v1, 0);
        assert_eq!(to_i64(obj_from_bits(v1)), Some(10));
        dec_ref_bits(_py, v1);

        let v2 = PyIter_Next(iter);
        assert_ne!(v2, 0);
        assert_eq!(to_i64(obj_from_bits(v2)), Some(20));
        dec_ref_bits(_py, v2);

        let v3 = PyIter_Next(iter);
        assert_ne!(v3, 0);
        assert_eq!(to_i64(obj_from_bits(v3)), Some(30));
        dec_ref_bits(_py, v3);

        // Exhausted — returns 0 with no exception.
        let v4 = PyIter_Next(iter);
        assert_eq!(v4, 0);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, iter);
        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn c_api_get_iter_on_non_iterable_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let iter = PyObject_GetIter(int_val);
        assert_eq!(iter, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_list_setitem_steals_ref_on_error() {
    let _guard = CApiTestGuard::new();
    // Verify that PyList_SetItem steals the reference even when the call fails.
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        // Try to SetItem on a dict (not a list) — should fail and steal the ref.
        let val = MoltObject::from_int(1).bits();
        inc_ref_bits(_py, val);
        let rc = PyList_SetItem(dict, 0, val);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, dict);
    });
}

// -----------------------------------------------------------------------
// Number Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_number_add_int() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(20).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(30));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_add_float() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_float(1.5).bits();
        let b = MoltObject::from_float(2.5).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        assert_eq!(obj_from_bits(res).as_float(), Some(4.0));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_subtract() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(50).bits();
        let b = MoltObject::from_int(30).bits();
        let res = PyNumber_Subtract(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(20));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_multiply() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(6).bits();
        let b = MoltObject::from_int(7).bits();
        let res = PyNumber_Multiply(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(42));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_truedivide() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(4).bits();
        let res = PyNumber_TrueDivide(a, b);
        assert_ne!(res, 0);
        assert_eq!(obj_from_bits(res).as_float(), Some(2.5));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_truedivide_by_zero() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(0).bits();
        let res = PyNumber_TrueDivide(a, b);
        assert_eq!(res, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_number_floordivide() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(17).bits();
        let b = MoltObject::from_int(5).bits();
        let res = PyNumber_FloorDivide(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(3));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_remainder() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(17).bits();
        let b = MoltObject::from_int(5).bits();
        let res = PyNumber_Remainder(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(2));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_power() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(2).bits();
        let b = MoltObject::from_int(10).bits();
        let res = PyNumber_Power(a, b, 0);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(1024));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_power_with_mod() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // pow(2, 10, 100) = 1024 % 100 = 24
        let a = MoltObject::from_int(2).bits();
        let b = MoltObject::from_int(10).bits();
        let m = MoltObject::from_int(100).bits();
        let res = PyNumber_Power(a, b, m);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(24));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_negative() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(42).bits();
        let res = PyNumber_Negative(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-42));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_positive() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(-7).bits();
        let res = PyNumber_Positive(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-7));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_absolute() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(-42).bits();
        let res = PyNumber_Absolute(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(42));
        dec_ref_bits(_py, res);

        let b = MoltObject::from_float(-3.125).bits();
        let res2 = PyNumber_Absolute(b);
        assert_ne!(res2, 0);
        assert_eq!(obj_from_bits(res2).as_float(), Some(3.125));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_invert() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(0).bits();
        let res = PyNumber_Invert(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-1));
        dec_ref_bits(_py, res);

        let b = MoltObject::from_int(7).bits();
        let res2 = PyNumber_Invert(b);
        assert_ne!(res2, 0);
        assert_eq!(to_i64(obj_from_bits(res2)), Some(-8));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_lshift_rshift() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(1).bits();
        let b = MoltObject::from_int(4).bits();
        let res = PyNumber_Lshift(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(16));
        dec_ref_bits(_py, res);

        let c = MoltObject::from_int(32).bits();
        let d = MoltObject::from_int(3).bits();
        let res2 = PyNumber_Rshift(c, d);
        assert_ne!(res2, 0);
        assert_eq!(to_i64(obj_from_bits(res2)), Some(4));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_and_or_xor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let a = MoltObject::from_int(0b1100).bits();
        let b = MoltObject::from_int(0b1010).bits();

        let and_res = PyNumber_And(a, b);
        assert_ne!(and_res, 0);
        assert_eq!(to_i64(obj_from_bits(and_res)), Some(0b1000));
        dec_ref_bits(_py, and_res);

        let or_res = PyNumber_Or(a, b);
        assert_ne!(or_res, 0);
        assert_eq!(to_i64(obj_from_bits(or_res)), Some(0b1110));
        dec_ref_bits(_py, or_res);

        let xor_res = PyNumber_Xor(a, b);
        assert_ne!(xor_res, 0);
        assert_eq!(to_i64(obj_from_bits(xor_res)), Some(0b0110));
        dec_ref_bits(_py, xor_res);
    });
}

#[test]
fn c_api_number_check() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        assert_eq!(PyNumber_Check(MoltObject::from_int(42).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::from_float(3.125).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::from_bool(true).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::none().bits()), 0);

        let str_ptr = alloc_string(_py, b"hello");
        assert!(!str_ptr.is_null());
        let str_bits = MoltObject::from_ptr(str_ptr).bits();
        assert_eq!(PyNumber_Check(str_bits), 0);
        dec_ref_bits(_py, str_bits);

        let list = PyList_New(0);
        assert_ne!(list, 0);
        assert_eq!(PyNumber_Check(list), 0);
        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_number_long_and_float() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // int(3.7) == 3
        let f = MoltObject::from_float(3.7).bits();
        let long_res = PyNumber_Long(f);
        assert_ne!(long_res, 0);
        assert_eq!(to_i64(obj_from_bits(long_res)), Some(3));
        dec_ref_bits(_py, long_res);

        // float(42) == 42.0
        let i = MoltObject::from_int(42).bits();
        let float_res = PyNumber_Float(i);
        assert_ne!(float_res, 0);
        assert_eq!(obj_from_bits(float_res).as_float(), Some(42.0));
        dec_ref_bits(_py, float_res);
    });
}

// -----------------------------------------------------------------------
// Mapping Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_mapping_length() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyMapping_Length(dict), 0);

        let key = MoltObject::from_int(1).bits();
        let val = MoltObject::from_int(100).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);
        assert_eq!(PyMapping_Length(dict), 1);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_mapping_keys_values_items() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let k1 = MoltObject::from_int(1).bits();
        let v1 = MoltObject::from_int(10).bits();
        let k2 = MoltObject::from_int(2).bits();
        let v2 = MoltObject::from_int(20).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

        let keys = PyMapping_Keys(dict);
        assert_ne!(keys, 0);
        assert_eq!(PySequence_Length(keys), 2);
        dec_ref_bits(_py, keys);

        let values = PyMapping_Values(dict);
        assert_ne!(values, 0);
        assert_eq!(PySequence_Length(values), 2);
        dec_ref_bits(_py, values);

        let items = PyMapping_Items(dict);
        assert_ne!(items, 0);
        assert_eq!(PySequence_Length(items), 2);
        dec_ref_bits(_py, items);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_mapping_getitemstring() {
    let _guard = CApiTestGuard::new();
    // PyMapping_GetItemString → molt_getitem_method → molt_index
    // each re-enter with_gil_entry!, producing 3 nested GIL frames.
    // In debug mode this overflows the 2MB default thread stack when
    // prior tests have consumed stack budget.  Run on a dedicated
    // thread with 8MB stack.
    let r = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .name("c_api_mapping_test".into())
        .spawn(|| {
            c_api_mapping_getitemstring_body();
        })
        .expect("spawn mapping test thread")
        .join();
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}

fn c_api_mapping_getitemstring_body() {
    let dict = PyDict_New();
    assert_ne!(dict, 0);

    // Set up the key via GIL entry (no deep nesting here).
    let key_bits_cell = std::cell::Cell::new(0u64);
    crate::with_gil_entry!(_py, {
        let key_ptr = alloc_string(_py, b"hello");
        assert!(!key_ptr.is_null());
        let val = MoltObject::from_int(99).bits();
        let kb = MoltObject::from_ptr(key_ptr).bits();
        assert_eq!(PyDict_SetItem(dict, kb, val), 0);
        key_bits_cell.set(kb);
    });
    let key_bits = key_bits_cell.get();

    // Call PyMapping_GetItemString outside with_gil_entry! to avoid
    // triple-nested GIL entry stack overflow.
    let got = unsafe { PyMapping_GetItemString(dict, c"hello".as_ptr()) };
    assert_ne!(got, 0);
    crate::with_gil_entry!(_py, {
        assert_eq!(to_i64(obj_from_bits(got)), Some(99));
        dec_ref_bits(_py, got);
    });

    // Missing key should fail.
    let missing = unsafe { PyMapping_GetItemString(dict, c"nope".as_ptr()) };
    assert_eq!(missing, 0);
    crate::with_gil_entry!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });

    // NULL key should fail.
    let null_key = unsafe { PyMapping_GetItemString(dict, std::ptr::null()) };
    assert_eq!(null_key, 0);
    crate::with_gil_entry!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_mapping_haskey() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let key = MoltObject::from_int(42).bits();
        let val = MoltObject::from_int(1).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);

        assert_eq!(PyMapping_HasKey(dict, key), 1);
        assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

        dec_ref_bits(_py, dict);
    });
}

// -----------------------------------------------------------------------
// Sequence Protocol addition tests
// -----------------------------------------------------------------------

#[test]
fn c_api_sequence_getitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(10).bits(),
                MoltObject::from_int(20).bits(),
                MoltObject::from_int(30).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        let item = PySequence_GetItem(list_bits, 1);
        assert_ne!(item, 0);
        assert_eq!(to_i64(obj_from_bits(item)), Some(20));
        dec_ref_bits(_py, item);

        // Negative index: -1 should get last element.
        let last = PySequence_GetItem(list_bits, -1);
        assert_ne!(last, 0);
        assert_eq!(to_i64(obj_from_bits(last)), Some(30));
        dec_ref_bits(_py, last);

        // Out-of-bounds.
        let bad = PySequence_GetItem(list_bits, 100);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn c_api_sequence_length() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        assert_eq!(PySequence_Length(list_bits), 2);

        let tuple = PyTuple_New(5);
        assert_ne!(tuple, 0);
        assert_eq!(PySequence_Length(tuple), 5);

        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_sequence_contains() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        assert_eq!(
            PySequence_Contains(list_bits, MoltObject::from_int(2).bits()),
            1
        );
        assert_eq!(
            PySequence_Contains(list_bits, MoltObject::from_int(9).bits()),
            0
        );

        dec_ref_bits(_py, list_bits);
    });
}

// -----------------------------------------------------------------------
// Bytes/String Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_bytes_from_string_and_size() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let data = b"hello bytes";
        let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
        assert_ne!(bytes, 0);

        let size = PyBytes_Size(bytes);
        assert_eq!(size, data.len() as isize);

        let ptr = PyBytes_AsString(bytes);
        assert!(!ptr.is_null());
        let observed = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
        assert_eq!(observed, data);

        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_bytes_empty() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 0) };
        assert_ne!(bytes, 0);
        assert_eq!(PyBytes_Size(bytes), 0);
        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_bytes_null_with_nonzero_len_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 5) };
        assert_eq!(bytes, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_negative_len_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(b"abc".as_ptr(), -1) };
        assert_eq!(bytes, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_asstring_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let ptr = PyBytes_AsString(int_val);
        assert!(ptr.is_null());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_size_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let size = PyBytes_Size(int_val);
        assert_eq!(size, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_from_string() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(c"hello world".as_ptr()) };
        assert_ne!(str_bits, 0);
        assert_eq!(PyUnicode_Check(str_bits), 1);

        let utf8_ptr = PyUnicode_AsUTF8(str_bits);
        assert!(!utf8_ptr.is_null());
        let observed = unsafe { std::ffi::CStr::from_ptr(utf8_ptr).to_bytes() };
        assert_eq!(observed, b"hello world");
        // The string content might not be NUL-terminated in molt's internal
        // storage, so compare the known length.
        let mut out_size: isize = 0;
        let utf8_ptr2 = unsafe { PyUnicode_AsUTF8AndSize(str_bits, &mut out_size as *mut isize) };
        assert!(!utf8_ptr2.is_null());
        assert_eq!(out_size, 11); // "hello world" is 11 bytes
        let observed2 =
            unsafe { std::slice::from_raw_parts(utf8_ptr2 as *const u8, out_size as usize) };
        assert_eq!(observed2, b"hello world");

        dec_ref_bits(_py, str_bits);
    });
}

#[test]
fn c_api_unicode_from_string_null_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(std::ptr::null()) };
        assert_eq!(str_bits, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_asutf8_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let ptr = PyUnicode_AsUTF8(int_val);
        assert!(ptr.is_null());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_asutf8andsize_null_size_ok() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(c"abc".as_ptr()) };
        assert_ne!(str_bits, 0);
        // Pass NULL for size — should not crash.
        let ptr = unsafe { PyUnicode_AsUTF8AndSize(str_bits, std::ptr::null_mut()) };
        assert!(!ptr.is_null());
        dec_ref_bits(_py, str_bits);
    });
}

// -----------------------------------------------------------------------
// Memory Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_pymem_malloc_realloc_free() {
    let _guard = CApiTestGuard::new();
    let ptr = unsafe { PyMem_Malloc(64) };
    assert!(!ptr.is_null());
    // Write to the allocated memory to verify it is usable.
    unsafe {
        std::ptr::write_bytes(ptr, 0xAB, 64);
        assert_eq!(*ptr, 0xAB);
    }
    let ptr2 = unsafe { PyMem_Realloc(ptr, 128) };
    assert!(!ptr2.is_null());
    // Original content should be preserved.
    unsafe {
        assert_eq!(*ptr2, 0xAB);
    }
    unsafe {
        PyMem_Free(ptr2);
    }
}

#[test]
fn c_api_pymem_malloc_zero_size() {
    let _guard = CApiTestGuard::new();
    // CPython returns a non-NULL pointer for size 0.
    let ptr = unsafe { PyMem_Malloc(0) };
    assert!(!ptr.is_null());
    unsafe {
        PyMem_Free(ptr);
    }
}

#[test]
fn c_api_pymem_free_null_is_safe() {
    let _guard = CApiTestGuard::new();
    // Freeing NULL should be a no-op.
    unsafe {
        PyMem_Free(std::ptr::null_mut());
    }
}

#[test]
fn c_api_pyobject_malloc_realloc_free() {
    let _guard = CApiTestGuard::new();
    let ptr = unsafe { PyObject_Malloc(32) };
    assert!(!ptr.is_null());
    unsafe {
        std::ptr::write_bytes(ptr, 0xCD, 32);
    }
    let ptr2 = unsafe { PyObject_Realloc(ptr, 64) };
    assert!(!ptr2.is_null());
    unsafe {
        assert_eq!(*ptr2, 0xCD);
    }
    unsafe {
        PyObject_Free(ptr2);
    }
}

#[test]
fn c_api_pyobject_free_null_is_safe() {
    let _guard = CApiTestGuard::new();
    // PyObject_Free delegates to PyMem_Free; NULL should be safe.
    unsafe {
        PyObject_Free(std::ptr::null_mut());
    }
}

// -----------------------------------------------------------------------
// Cross-protocol integration tests
// -----------------------------------------------------------------------

#[test]
fn c_api_number_mixed_int_float_arithmetic() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // int + float -> float
        let a = MoltObject::from_int(3).bits();
        let b = MoltObject::from_float(0.125).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        let val = obj_from_bits(res).as_float().unwrap();
        assert!((val - 3.125).abs() < 1e-10);
        dec_ref_bits(_py, res);

        // float * int -> float
        let c = MoltObject::from_float(2.5).bits();
        let d = MoltObject::from_int(4).bits();
        let res2 = PyNumber_Multiply(c, d);
        assert_ne!(res2, 0);
        assert_eq!(obj_from_bits(res2).as_float(), Some(10.0));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_sequence_and_mapping_on_dict() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let k1 = MoltObject::from_int(1).bits();
        let v1 = MoltObject::from_int(100).bits();
        let k2 = MoltObject::from_int(2).bits();
        let v2 = MoltObject::from_int(200).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

        // PyMapping_Length works on dict.
        assert_eq!(PyMapping_Length(dict), 2);

        // PyMapping_HasKey works.
        assert_eq!(PyMapping_HasKey(dict, k1), 1);
        assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

        // PySequence_Contains also works on dict (checks keys).
        assert_eq!(PySequence_Contains(dict, k2), 1);
        assert_eq!(
            PySequence_Contains(dict, MoltObject::from_int(999).bits()),
            0
        );

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_bytes_roundtrip_via_protocol() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let data = b"\x00\x01\x02\xff";
        let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
        assert_ne!(bytes, 0);
        assert_eq!(PyBytes_Size(bytes), 4);
        let ptr = PyBytes_AsString(bytes);
        assert!(!ptr.is_null());
        let observed = unsafe { std::slice::from_raw_parts(ptr, 4) };
        assert_eq!(observed, data);
        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_object_protocol_repr_str_hash_truthy() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let int_val = MoltObject::from_int(42).bits();

        // PyObject_Repr — re-entrant GIL acquisition
        let repr = PyObject_Repr(int_val);
        assert_ne!(repr, 0);
        dec_ref_bits(_py, repr);

        // PyObject_Str
        let str_val = PyObject_Str(int_val);
        assert_ne!(str_val, 0);
        dec_ref_bits(_py, str_val);

        // PyObject_Hash
        let hash = PyObject_Hash(int_val);
        assert_ne!(hash, -1);

        // PyObject_IsTrue / PyObject_Not
        assert_eq!(PyObject_IsTrue(int_val), 1);
        assert_eq!(PyObject_Not(int_val), 0);
        assert_eq!(PyObject_IsTrue(MoltObject::from_int(0).bits()), 0);
        assert_eq!(PyObject_Not(MoltObject::from_int(0).bits()), 1);
        assert_eq!(PyObject_IsTrue(MoltObject::from_bool(true).bits()), 1);
        assert_eq!(PyObject_IsTrue(MoltObject::from_bool(false).bits()), 0);
    });
}

#[test]
fn c_api_object_type_and_length() {
    let _guard = CApiTestGuard::new();
    // C-API functions acquire GIL internally — don't nest
    let list = PyList_New(3);
    assert_ne!(list, 0);

    let ty = PyObject_Type(list);
    assert_ne!(ty, 0);
    crate::with_gil_entry!(_py, { dec_ref_bits(_py, ty) });

    assert_eq!(PyObject_Length(list), 3);
    assert_eq!(PyObject_Size(list), 3);

    crate::with_gil_entry!(_py, { dec_ref_bits(_py, list) });
}

#[test]
fn c_api_rich_compare() {
    let _guard = CApiTestGuard::new();
    let a = MoltObject::from_int(10).bits();
    let b = MoltObject::from_int(20).bits();

    assert_eq!(PyObject_RichCompareBool(a, b, 0), 1); // 10 < 20
    assert_eq!(PyObject_RichCompareBool(a, b, 1), 1); // 10 <= 20
    assert_eq!(PyObject_RichCompareBool(a, b, 2), 0); // 10 == 20
    assert_eq!(PyObject_RichCompareBool(a, b, 3), 1); // 10 != 20
    assert_eq!(PyObject_RichCompareBool(a, b, 4), 0); // 10 > 20
    assert_eq!(PyObject_RichCompareBool(a, b, 5), 0); // 10 >= 20

    // Invalid op
    assert_eq!(PyObject_RichCompareBool(a, b, 99), -1);
    crate::with_gil_entry!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });

    let cmp = PyObject_RichCompare(a, b, 2);
    assert_ne!(cmp, 0);
    crate::with_gil_entry!(_py, { dec_ref_bits(_py, cmp) });
}

#[test]
fn c_api_callable_check_and_isinstance() {
    let _guard = CApiTestGuard::new();
    let int_val = MoltObject::from_int(5).bits();
    assert_eq!(PyCallable_Check(int_val), 0);

    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let int_type = builtins.int;
        let result = PyObject_IsInstance(int_val, int_type);
        assert_eq!(result, 1);

        let none_val = none_bits();
        let result2 = PyObject_IsInstance(none_val, int_type);
        assert_eq!(result2, 0);
    });
}

#[test]
fn c_api_set_protocol() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        // Create empty set — capacity is raw u64, NOT NaN-boxed
        let set = molt_set_new(0u64);
        assert!(!obj_from_bits(set).is_none());

        // PySet_Check / PyFrozenSet_Check
        assert_eq!(PySet_Check(set), 1);
        assert_eq!(PyFrozenSet_Check(set), 0);

        // Add elements via runtime directly
        let k1 = MoltObject::from_int(10).bits();
        let k2 = MoltObject::from_int(20).bits();
        let add_res1 = molt_set_add(set, k1);
        assert!(!exception_pending(_py));
        if !obj_from_bits(add_res1).is_none() {
            dec_ref_bits(_py, add_res1);
        }
        let add_res2 = molt_set_add(set, k2);
        if !obj_from_bits(add_res2).is_none() {
            dec_ref_bits(_py, add_res2);
        }

        // PySet_Size
        assert_eq!(PySet_Size(set), 2);

        // PySet_Contains
        assert_eq!(PySet_Contains(set, k1), 1);
        assert_eq!(PySet_Contains(set, MoltObject::from_int(99).bits()), 0);

        // Discard
        let disc_res = molt_set_discard(set, k1);
        if !obj_from_bits(disc_res).is_none() {
            dec_ref_bits(_py, disc_res);
        }
        assert_eq!(PySet_Contains(set, k1), 0);

        // Pop
        let popped = PySet_Pop(set);
        assert_ne!(popped, 0);
        assert_eq!(PySet_Size(set), 0);
        dec_ref_bits(_py, popped);

        // Clear
        let add_res3 = molt_set_add(set, k1);
        if !obj_from_bits(add_res3).is_none() {
            dec_ref_bits(_py, add_res3);
        }
        assert_eq!(PySet_Clear(set), 0);
        assert_eq!(PySet_Size(set), 0);

        dec_ref_bits(_py, set);
    });
}

#[test]
fn c_api_dict_extended_operations() {
    let _guard = CApiTestGuard::new();
    let dict = PyDict_New();
    assert_ne!(dict, 0);

    crate::with_gil_entry!(_py, {
        let k1_ptr = alloc_string(_py, b"hello");
        assert!(!k1_ptr.is_null());
        let k1 = MoltObject::from_ptr(k1_ptr).bits();
        let v1 = MoltObject::from_int(100).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);

        let got = PyDict_GetItemString(dict, c"hello".as_ptr());
        assert_ne!(got, 0);

        assert_eq!(PyDict_DelItem(dict, k1), 0);
        assert_eq!(PyDict_Size(dict), 0);

        let rc = PyDict_DelItem(dict, k1);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        let keys = PyDict_Keys(dict);
        assert_ne!(keys, 0);
        dec_ref_bits(_py, keys);
        let vals = PyDict_Values(dict);
        assert_ne!(vals, 0);
        dec_ref_bits(_py, vals);
        let items = PyDict_Items(dict);
        assert_ne!(items, 0);
        dec_ref_bits(_py, items);

        let copy = PyDict_Copy(dict);
        assert_ne!(copy, 0);
        assert_eq!(PyDict_Size(copy), 1);
        dec_ref_bits(_py, copy);

        dec_ref_bits(_py, k1);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_list_extended_operations() {
    let _guard = CApiTestGuard::new();
    let list = PyList_New(0);
    assert_ne!(list, 0);

    assert_eq!(PyList_Append(list, MoltObject::from_int(3).bits()), 0);
    assert_eq!(PyList_Append(list, MoltObject::from_int(1).bits()), 0);
    assert_eq!(PyList_Append(list, MoltObject::from_int(2).bits()), 0);
    assert_eq!(PyList_Size(list), 3);

    assert_eq!(PyList_Insert(list, 0, MoltObject::from_int(0).bits()), 0);
    assert_eq!(PyList_Size(list), 4);

    assert_eq!(PyList_Reverse(list), 0);
    assert_eq!(PyList_Sort(list), 0);

    let tup = PyList_AsTuple(list);
    assert_ne!(tup, 0);
    assert_eq!(PyTuple_Size(tup), 4);
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, tup);
        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_exception_protocol() {
    let _guard = CApiTestGuard::new();
    assert_eq!(PyErr_Occurred(), 0);

    PyErr_SetString(0, c"test error".as_ptr());
    assert_ne!(PyErr_Occurred(), 0);

    PyErr_Clear();
    assert_eq!(PyErr_Occurred(), 0);

    let _ = PyErr_NoMemory();
    assert_ne!(PyErr_Occurred(), 0);
    PyErr_Clear();
}

#[test]
fn c_api_refcount_and_conversions() {
    let _guard = CApiTestGuard::new();
    // PyLong_FromLong / PyLong_AsLong — inline NaN-boxed, no GIL needed
    let long = PyLong_FromLong(42);
    assert_ne!(long, 0);
    assert_eq!(PyLong_AsLong(long), 42);

    let float = PyFloat_FromDouble(3.125);
    let val = PyFloat_AsDouble(float);
    assert!((val - 3.125).abs() < 0.001);

    let t = PyBool_FromLong(1);
    assert_eq!(PyObject_IsTrue(t), 1);
    let f = PyBool_FromLong(0);
    assert_eq!(PyObject_IsTrue(f), 0);

    let n = Py_BuildNone();
    assert!(obj_from_bits(n).is_none());

    crate::with_gil_entry!(_py, {
        let s_ptr = alloc_string(_py, b"refcount_test");
        assert!(!s_ptr.is_null());
        let s = MoltObject::from_ptr(s_ptr).bits();
        Py_IncRef(s);
        Py_DecRef(s);
        Py_XINCREF(s);
        Py_XDECREF(s);
        dec_ref_bits(_py, s);
    });
}

#[test]
fn c_api_unicode_extended() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry!(_py, {
        let s_ptr = alloc_string(_py, b"hello");
        assert!(!s_ptr.is_null());
        let s = MoltObject::from_ptr(s_ptr).bits();

        assert_eq!(PyUnicode_GetLength(s), 5);

        let sub_ptr = alloc_string(_py, b"ell");
        assert!(!sub_ptr.is_null());
        let sub = MoltObject::from_ptr(sub_ptr).bits();
        assert_eq!(PyUnicode_Contains(s, sub), 1);

        let s2_ptr = alloc_string(_py, b" world");
        assert!(!s2_ptr.is_null());
        let s2 = MoltObject::from_ptr(s2_ptr).bits();
        let concat = PyUnicode_Concat(s, s2);
        assert_ne!(concat, 0);
        assert_eq!(PyUnicode_GetLength(concat), 11);
        dec_ref_bits(_py, concat);

        let cmp = PyUnicode_CompareWithASCIIString(s, c"hello".as_ptr());
        assert_eq!(cmp, 0);

        dec_ref_bits(_py, s2);
        dec_ref_bits(_py, sub);
        dec_ref_bits(_py, s);
    });
}
