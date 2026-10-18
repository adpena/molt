use super::bridge_intern_static_name;
use crate::{clear_exception, exception_pending, runtime_state};
use std::sync::atomic::Ordering;

#[test]
fn converter_replacement_retires_raised_owner_and_keeps_handled_context() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, ExceptionStackScope, ExceptionValue, alloc_exception,
            exception_context_active_bits, exception_context_set, exception_field,
            exception_matches_builtin_name, molt_exception_last_pending,
        };
        let outer = alloc_exception(py, "LookupError", "outer handler");
        assert!(!outer.is_null());
        let outer = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(outer).bits());
        let handler = ExceptionStackScope::push(py);
        exception_context_set(py, outer.bits());
        let old = alloc_exception(py, "ValueError", "exporter refused");
        assert!(!old.is_null());
        let old = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(old).bits());
        let refs = unsafe {
            (*crate::header_from_obj_ptr(crate::ptr_from_bits(old.bits()))).ref_count_snapshot()
        };
        crate::builtins::exceptions::molt_raise(old.bits());
        let kind = b"TypeError";
        let message = b"converter rejected input";
        let returned = unsafe {
            (super::RUNTIME_VTABLE.replace_exception)(
                kind.as_ptr(),
                kind.len(),
                message.as_ptr(),
                message.len(),
            )
        };
        assert_eq!(returned, crate::MoltObject::none().bits());
        assert!(exception_pending(py));
        let selected = ExceptionValue::adopt(py, molt_exception_last_pending());
        assert!(exception_matches_builtin_name(
            py,
            selected.bits(),
            "TypeError"
        ));
        assert_ne!(selected.bits(), old.bits());
        assert_eq!(
            exception_field(py, selected.bits(), ExceptionFieldSlot::Context)
                .unwrap()
                .bits(),
            outer.bits()
        );
        assert_eq!(exception_context_active_bits(), Some(outer.bits()));
        assert_eq!(
            unsafe {
                (*crate::header_from_obj_ptr(crate::ptr_from_bits(old.bits()))).ref_count_snapshot()
            },
            refs
        );
        clear_exception(py);
        drop(handler);
    });
}

#[test]
fn binascii_exception_declarations_survive_runtime_rebootstrap() {
    for _ in 0..2 {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                use crate::builtins::exceptions::{ExceptionValue, molt_exception_last_pending};
                let state = runtime_state(py);
                assert!(
                    !state
                        .exception_type_cache
                        .lock()
                        .unwrap()
                        .contains_key("binascii.Error")
                );
                let error = ExceptionValue::adopt(
                    py,
                    molt_runtime_serial::binascii::molt_binascii_error_type(),
                );
                let incomplete = ExceptionValue::adopt(
                    py,
                    molt_runtime_serial::binascii::molt_binascii_incomplete_type(),
                );
                assert!(!exception_pending(py));
                assert_ne!(error.bits(), incomplete.bits());
                let error_ptr = crate::obj_from_bits(error.bits()).as_ptr().unwrap();
                let baseline =
                    unsafe { (*crate::header_from_obj_ptr(error_ptr)).ref_count_snapshot() };
                {
                    let same = ExceptionValue::adopt(
                        py,
                        molt_runtime_serial::binascii::molt_binascii_error_type(),
                    );
                    assert_eq!(same.bits(), error.bits());
                    assert_eq!(
                        unsafe { (*crate::header_from_obj_ptr(error_ptr)).ref_count_snapshot() },
                        baseline + 1
                    );
                }
                assert_eq!(
                    unsafe { (*crate::header_from_obj_ptr(error_ptr)).ref_count_snapshot() },
                    baseline
                );
                let data = crate::alloc_bytes(py, b"A");
                assert!(!data.is_null());
                let data = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(data).bits());
                molt_runtime_serial::binascii::molt_binascii_a2b_base64(
                    data.bits(),
                    crate::MoltObject::from_bool(false).bits(),
                );
                assert!(exception_pending(py));
                let raised = ExceptionValue::adopt(py, molt_exception_last_pending());
                assert_eq!(crate::type_of_bits(py, raised.bits()), error.bits());
                clear_exception(py);
            });
        });
    }
}

#[test]
fn ascii_decode_error_retains_real_codec_context_and_outer_handler() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, ExceptionStackScope, ExceptionValue, alloc_exception,
            exception_context_active_bits, exception_context_set, exception_field,
            exception_matches_builtin_name, exception_suppress_bits, molt_exception_last_pending,
        };
        let text = crate::alloc_string(py, "hé🙂".as_bytes());
        assert!(!text.is_null());
        let text = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(text).bits());
        let outer = alloc_exception(py, "LookupError", "outer handler");
        assert!(!outer.is_null());
        let outer = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(outer).bits());
        let handler = ExceptionStackScope::push(py);
        exception_context_set(py, outer.bits());
        let returned = unsafe { (super::RUNTIME_VTABLE.decode_ascii_input)(text.bits()) };
        assert_eq!(returned, crate::MoltObject::none().bits());
        assert!(exception_pending(py));
        let translated = ExceptionValue::adopt(py, molt_exception_last_pending());
        assert!(exception_matches_builtin_name(
            py,
            translated.bits(),
            "ValueError"
        ));
        let context = exception_field(py, translated.bits(), ExceptionFieldSlot::Context).unwrap();
        assert!(exception_matches_builtin_name(
            py,
            context.bits(),
            "UnicodeEncodeError"
        ));
        assert_eq!(
            exception_field(py, context.bits(), ExceptionFieldSlot::Context)
                .unwrap()
                .bits(),
            outer.bits()
        );
        let args = exception_field(py, context.bits(), ExceptionFieldSlot::Args).unwrap();
        let args = crate::obj_from_bits(args.bits()).as_ptr().unwrap();
        unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(args, |items| {
                assert_eq!(items.len(), 5);
                assert_eq!(
                    crate::string_obj_to_owned(crate::obj_from_bits(items[0])).as_deref(),
                    Some("ascii")
                );
                assert_eq!(items[1], text.bits());
                assert_eq!(crate::to_i64(crate::obj_from_bits(items[2])), Some(1));
                assert_eq!(crate::to_i64(crate::obj_from_bits(items[3])), Some(3));
                assert_eq!(
                    crate::string_obj_to_owned(crate::obj_from_bits(items[4])).as_deref(),
                    Some("ordinal not in range(128)")
                );
            })
        }
        .unwrap();
        assert!(
            crate::obj_from_bits(
                exception_field(py, translated.bits(), ExceptionFieldSlot::Cause)
                    .unwrap()
                    .bits()
            )
            .is_none()
        );
        let translated_pointer = crate::obj_from_bits(translated.bits()).as_ptr().unwrap();
        assert_eq!(
            unsafe { exception_suppress_bits(translated_pointer) },
            crate::MoltObject::from_bool(false).bits()
        );
        assert_eq!(exception_context_active_bits(), Some(outer.bits()));
        clear_exception(py);
        drop(handler);
        assert!(!exception_pending(py));
    });
}

#[test]
#[cfg(feature = "stdlib_csv")]
fn csv_domain_errors_use_supplied_class_and_borrow_constructor_arguments() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::{ExceptionValue, exception_type_bits_from_name};
        let class = exception_type_bits_from_name(py, "RuntimeError");
        let message_ptr = crate::alloc_string(py, b"csv owned message");
        assert!(!message_ptr.is_null());
        let message = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(message_ptr).bits());
        let tuple_ptr = crate::alloc_tuple(py, &[message.bits()]);
        assert!(!tuple_ptr.is_null());
        let tuple = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(tuple_ptr).bits());
        let refs = unsafe { (*crate::header_from_obj_ptr(tuple_ptr)).ref_count_snapshot() };
        {
            let exception = ExceptionValue::adopt(py, unsafe {
                (super::RUNTIME_VTABLE.exception_new_from_class)(class, tuple.bits())
            });
            assert!(!crate::exception_pending(py));
            assert_eq!(crate::type_of_bits(py, exception.bits()), class);
            assert!(
                unsafe { (*crate::header_from_obj_ptr(tuple_ptr)).ref_count_snapshot() } > refs
            );
        }
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(tuple_ptr)).ref_count_snapshot() },
            refs
        );
        let missing_ptr = crate::alloc_string(py, b"molt_missing_csv_domain_test");
        assert!(!missing_ptr.is_null());
        let missing = ExceptionValue::adopt(py, crate::MoltObject::from_ptr(missing_ptr).bits());
        let result = molt_runtime_serial::csv::molt_csv_get_dialect(missing.bits(), class);
        assert_eq!(result, crate::MoltObject::none().bits());
        assert!(crate::exception_pending(py));
        let raised = crate::builtins::exceptions::thread_last_exception_bits_noinc(py).unwrap();
        assert_eq!(crate::type_of_bits(py, raised), class);
        crate::clear_exception(py);
    });
}

#[test]
fn serial_bridge_interns_protocol_names_in_runtime_state() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        clear_exception(_py);
        let key = b"__abs__";
        let bits = bridge_intern_static_name(key.as_ptr(), key.len());
        assert!(!exception_pending(_py));
        assert_eq!(
            runtime_state(_py).interned.abs_name.load(Ordering::Acquire),
            bits
        );
    });
}

#[test]
fn serial_bridge_rejects_unknown_intern_name() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        clear_exception(_py);
        let key = b"__molt_unknown__";
        let _ = bridge_intern_static_name(key.as_ptr(), key.len());
        assert!(exception_pending(_py));
        clear_exception(_py);
    });
}

#[test]
fn serial_identity_returns_keep_independent_owners() {
    use crate::*;
    use molt_runtime_serial::{configparser, csv};

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        // Keep the child independently owned so final container retirement
        // can be observed without reading a freed container header.
        let child = alloc_list(py, &[]);
        assert!(!child.is_null());
        let child_bits = MoltObject::from_ptr(child).bits();
        let child_refs = unsafe { (*header_from_obj_ptr(child)).ref_count_snapshot() };
        for tuple in [false, true] {
            for release_input_first in [false, true] {
                let row = if tuple {
                    alloc_tuple(py, &[child_bits])
                } else {
                    alloc_list(py, &[child_bits])
                };
                assert!(!row.is_null());
                let bits = MoltObject::from_ptr(row).bits();
                let before = unsafe { (*header_from_obj_ptr(row)).ref_count_snapshot() };
                let result = csv::molt_csv_normalize_row(bits);
                assert!(!exception_pending(py));
                assert_eq!(result, bits);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(row)).ref_count_snapshot() },
                    before + 1,
                );
                let (first, survivor) = if release_input_first {
                    (bits, result)
                } else {
                    (result, bits)
                };
                dec_ref_bits(py, first);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(row)).ref_count_snapshot() },
                    before,
                );
                dec_ref_bits(py, survivor);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(child)).ref_count_snapshot() },
                    child_refs,
                );
            }
        }
        let none = MoltObject::none().bits();
        assert_eq!(
            csv::molt_csv_normalize_row(MoltObject::from_int(7).bits()),
            none
        );
        assert!(!exception_pending(py));
        let handle = configparser::molt_configparser_new(none, none);
        assert!(!exception_pending(py));
        let section = alloc_string(py, b"missing-section");
        let option = alloc_string(py, b"missing-option");
        assert!(!section.is_null() && !option.is_null());
        let section = MoltObject::from_ptr(section).bits();
        let option = MoltObject::from_ptr(option).bits();
        let getters: [extern "C" fn(u64, u64, u64, u64) -> u64; 4] = [
            configparser::molt_configparser_get,
            configparser::molt_configparser_getint,
            configparser::molt_configparser_getfloat,
            configparser::molt_configparser_getboolean,
        ];
        for get in getters {
            let result = get(handle, section, option, child_bits);
            assert!(!exception_pending(py));
            assert_eq!(result, child_bits);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(child)).ref_count_snapshot() },
                child_refs + 1,
            );
            dec_ref_bits(py, result);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(child)).ref_count_snapshot() },
                child_refs,
            );
            let missing = get(handle, section, option, none);
            assert!(exception_pending(py));
            clear_exception(py);
            dec_ref_bits(py, missing);
        }
        configparser::molt_configparser_drop(handle);
        dec_ref_bits(py, handle);
        dec_ref_bits(py, section);
        dec_ref_bits(py, option);
        dec_ref_bits(py, child_bits);
    });
}
