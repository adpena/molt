//! Native `BaseExceptionGroup.__new__` consumes the one runtime group
//! admission authority through `RuntimeHooks::exception_group_admit`.

use super::*;
use crate::builtins::exceptions::alloc_exception;
use crate::object::builders::alloc_tuple;
use molt_cpython_abi::abi_types::{
    Py_ssize_t, PyBaseExceptionObject, PyExc_BaseExceptionGroup, PyExc_ValueError, PyObject,
    PyTypeObject,
};
use molt_cpython_abi::api::{errors, object, refcount, sequences, strings, typeobj};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::ExceptionGroupRequest;

fn exception(py: &crate::PyToken<'_>, kind: &str) -> u64 {
    let ptr = alloc_exception(py, kind, "admission");
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

/// `("admitted", items)` with an exact items tuple. Returns owned `args` and an
/// owned reference to the items tuple.
fn group_args(py: &crate::PyToken<'_>, items: &[u64]) -> (u64, u64) {
    let message = alloc_string(py, b"admitted");
    let tuple = alloc_tuple(py, items);
    assert!(!message.is_null() && !tuple.is_null());
    let message = MoltObject::from_ptr(message).bits();
    let tuple = MoltObject::from_ptr(tuple).bits();
    let args = alloc_tuple(py, &[message, tuple]);
    assert!(!args.is_null());
    dec_ref_bits(py, message);
    (MoltObject::from_ptr(args).bits(), tuple)
}

fn admit(request: u32, name: &CStr, args: u64) -> (c_int, u64, u64) {
    let mut message = 0;
    let mut exceptions = 0;
    let status = unsafe {
        hook_exception_group_admit(
            request,
            name.as_ptr(),
            args,
            &raw mut message,
            &raw mut exceptions,
        )
    };
    (status, message, exceptions)
}

fn take_pending(py: &crate::PyToken<'_>, kind: &str) -> String {
    let error = crate::exception_last_bits_noinc(py).expect("pending exception");
    assert_eq!(crate::type_name(py, crate::obj_from_bits(error)), kind);
    let text = crate::builtins::exceptions::format_exception_message(
        py,
        crate::obj_from_bits(error)
            .as_ptr()
            .expect("exception object"),
    );
    crate::clear_exception(py);
    text
}

fn ref_count(bits: u64) -> u32 {
    let ptr = crate::obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

#[test]
fn hook_admission_reports_exact_cpython_decisions() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let value = exception(py, "ValueError");
        let interrupt = exception(py, "KeyboardInterrupt");

        let (args, tuple) = group_args(py, &[value]);
        let value_baseline = ref_count(value);
        let (status, message, exceptions) = admit(
            ExceptionGroupRequest::BaseExceptionGroup as u32,
            c"BaseExceptionGroup",
            args,
        );
        assert_eq!(
            status, 1,
            "exact BaseExceptionGroup narrows for Exception items"
        );
        assert!(!crate::exception_pending(py));
        assert_eq!(exceptions, tuple, "PySequence_Tuple keeps an exact tuple");
        assert_eq!(
            crate::string_obj_to_owned(crate::obj_from_bits(message)).as_deref(),
            Some("admitted")
        );
        dec_ref_bits(py, message);
        dec_ref_bits(py, exceptions);
        assert_eq!(ref_count(value), value_baseline);

        let (mixed_args, mixed_tuple) = group_args(py, &[value, interrupt]);
        for request in [
            ExceptionGroupRequest::BaseExceptionGroup,
            ExceptionGroupRequest::BaseExceptionSubclass,
        ] {
            let (status, message, exceptions) = admit(request as u32, c"ext.Group", mixed_args);
            assert_eq!(status, 0, "{request:?} keeps its requested type");
            assert_eq!(exceptions, mixed_tuple);
            dec_ref_bits(py, message);
            dec_ref_bits(py, exceptions);
        }
        for (request, expected) in [
            (
                ExceptionGroupRequest::ExceptionGroup,
                "Cannot nest BaseExceptions in an ExceptionGroup",
            ),
            (
                ExceptionGroupRequest::ExceptionSubclass,
                "Cannot nest BaseExceptions in 'ext.Group'",
            ),
        ] {
            let (status, message, exceptions) = admit(request as u32, c"ext.Group", mixed_args);
            assert_eq!((status, message, exceptions), (-1, 0, 0));
            assert_eq!(take_pending(py, "TypeError"), expected);
        }

        // CPython formats the requested tp_name with %.200s.
        let long_name = std::ffi::CString::new("N".repeat(250)).unwrap();
        let (status, _, _) = admit(
            ExceptionGroupRequest::ExceptionSubclass as u32,
            &long_name,
            mixed_args,
        );
        assert_eq!(status, -1);
        assert_eq!(
            take_pending(py, "TypeError"),
            format!("Cannot nest BaseExceptions in '{}'", "N".repeat(200))
        );

        // An unknown request is an ABI defect the C allocator reports.
        assert_eq!(admit(9, c"ext.Group", mixed_args), (-1, 0, 0));
        assert!(!crate::exception_pending(py));

        let text = MoltObject::from_ptr(alloc_string(py, b"not an exception")).bits();
        let (bad_args, bad_tuple) = group_args(py, &[value, text]);
        assert_eq!(
            admit(
                ExceptionGroupRequest::BaseExceptionGroup as u32,
                c"BaseExceptionGroup",
                bad_args
            ),
            (-1, 0, 0)
        );
        assert_eq!(
            take_pending(py, "ValueError"),
            "Item 1 of second argument (exceptions) is not an exception"
        );

        let short = alloc_tuple(py, &[text]);
        assert!(!short.is_null());
        let short = MoltObject::from_ptr(short).bits();
        assert_eq!(
            admit(
                ExceptionGroupRequest::BaseExceptionGroup as u32,
                c"BaseExceptionGroup",
                short
            ),
            (-1, 0, 0)
        );
        assert_eq!(
            take_pending(py, "TypeError"),
            "BaseExceptionGroup.__new__() takes exactly 2 arguments (1 given)"
        );

        for bits in [
            short,
            bad_args,
            bad_tuple,
            text,
            mixed_args,
            mixed_tuple,
            args,
            tuple,
            interrupt,
            value,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

unsafe fn pair(first: *mut PyObject, second: *mut PyObject) -> *mut PyObject {
    unsafe {
        let pair = sequences::PyTuple_New(2);
        assert!(!pair.is_null());
        for (index, item) in [first, second].into_iter().enumerate() {
            refcount::Py_INCREF(item);
            assert_eq!(
                sequences::PyTuple_SetItem(pair, index as Py_ssize_t, item),
                0
            );
        }
        pair
    }
}

unsafe fn take_c_error(expected: *mut PyTypeObject) -> String {
    unsafe {
        let pending =
            crate::with_gil(|py| crate::builtins::exceptions::pending_exception_diagnostic(&py));
        let observed = errors::PyErr_Occurred();
        assert_eq!(
            errors::PyErr_ExceptionMatches(expected.cast()),
            1,
            "runtime pending: {pending:?}; pending C class: {:?}",
            (!observed.is_null())
                .then(|| CStr::from_ptr((*observed.cast::<PyTypeObject>()).tp_name))
        );
        let raised = errors::PyErr_GetRaisedException();
        let raised_owner = refcount::OwnedPyObject::from_owned(raised);
        assert!(!raised.is_null());
        let text = typeobj::PyObject_Str(raised);
        let text_owner = refcount::OwnedPyObject::from_owned(text);
        assert!(!text.is_null());
        let utf8 = strings::PyUnicode_AsUTF8(text);
        assert!(!utf8.is_null());
        let rendered = CStr::from_ptr(utf8).to_string_lossy().into_owned();
        drop(text_owner);
        drop(raised_owner);
        rendered
    }
}

#[test]
fn native_group_allocation_consumes_runtime_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    // The native caller owns one execution boundary across construction and
    // observation of its error, as a C extension does while holding the GIL.
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let text = strings::PyUnicode_FromString(c"native".as_ptr());
            assert!(!text.is_null());
            let value = object::PyObject_CallOneArg((&raw mut PyExc_ValueError).cast(), text);
            assert!(!value.is_null());
            let children = sequences::PyTuple_New(1);
            assert!(!children.is_null());
            assert_eq!(sequences::PyTuple_SetItem(children, 0, value), 0);
            let args = pair(text, children);

            let group = errors::molt_native_exception_new(
                &raw mut PyExc_BaseExceptionGroup,
                args,
                ptr::null_mut(),
            );
            assert!(!group.is_null());
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(
                CStr::from_ptr((*(*group).ob_type).tp_name),
                c"ExceptionGroup",
                "exact BaseExceptionGroup narrows for Exception items"
            );
            let field = molt_cpython_abi::abi_types::exception_typed_object_slot(
                group.cast::<PyBaseExceptionObject>(),
                molt_obj_model::ExceptionLayoutKind::Group,
                molt_obj_model::ExceptionTypedField::GroupExceptions,
            )
            .expect("group exceptions slot");
            assert_eq!(*field, children, "the exact tuple is the exceptions field");
            refcount::Py_DECREF(group);

            // Item identity is the real native/managed type, with CPython's text.
            let bad_children = pair(value, text);
            let bad_args = pair(text, bad_children);
            let rejected = errors::molt_native_exception_new(
                &raw mut PyExc_BaseExceptionGroup,
                bad_args,
                ptr::null_mut(),
            );
            assert!(rejected.is_null());
            assert_eq!(
                take_c_error(&raw mut PyExc_ValueError),
                "Item 1 of second argument (exceptions) is not an exception"
            );

            for object in [bad_args, bad_children, args, children, text] {
                refcount::Py_DECREF(object);
            }
        }
    });
}

extern "C" fn derive_native_group(children: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let message = MoltObject::from_ptr(alloc_string(py, b"derived-native")).bits();
            let args = MoltObject::from_ptr(alloc_tuple(py, &[message, children])).bits();
            let native = errors::molt_native_exception_new(
                &raw mut PyExc_BaseExceptionGroup,
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(args),
                ptr::null_mut(),
            );
            dec_ref_bits(py, args);
            dec_ref_bits(py, message);
            assert!(!native.is_null());
            let result = GLOBAL_BRIDGE.molt_value_for_pyobj(native).unwrap();
            refcount::Py_DECREF(native);
            result
        }
    })
}

#[test]
fn native_group_nodes_use_public_split_binding_and_preserve_leaf_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let empty = sequences::PyTuple_New(0);
            assert!(!empty.is_null());
            let native_value = errors::molt_native_exception_new(
                &raw mut PyExc_ValueError,
                empty,
                ptr::null_mut(),
            );
            assert!(!native_value.is_null());
            refcount::Py_DECREF(empty);
            let value = GLOBAL_BRIDGE.molt_value_for_pyobj(native_value).unwrap();
            refcount::Py_DECREF(native_value);
            let other = exception(py, "TypeError");
            let (args, children) = group_args(py, &[value, other]);
            let c_args = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(args);
            let native_group = errors::molt_native_exception_new(
                &raw mut PyExc_BaseExceptionGroup,
                c_args,
                ptr::null_mut(),
            );
            assert!(!native_group.is_null());
            let group = GLOBAL_BRIDGE.molt_value_for_pyobj(native_group).unwrap();
            refcount::Py_DECREF(native_group);
            assert!(errors::PyErr_Occurred().is_null());

            let split_name = crate::attr_name_bits_from_bytes(py, b"split").unwrap();
            let exceptions_name = crate::attr_name_bits_from_bytes(py, b"exceptions").unwrap();
            let notes_name = crate::attr_name_bits_from_bytes(py, b"__notes__").unwrap();
            let note = MoltObject::from_ptr(alloc_string(py, b"kept")).bits();
            let notes = MoltObject::from_ptr(crate::alloc_list(py, &[note])).bits();
            let set_result = crate::molt_set_attr_name(group, notes_name, notes);
            dec_ref_bits(py, set_result);
            let callable = crate::molt_get_attr_name(
                crate::builtin_classes(py).base_exception_group,
                split_name,
            );
            assert!(!crate::exception_pending(py));
            let value_class =
                crate::builtins::exceptions::exception_type_bits_from_name(py, "ValueError");
            assert!(
                crate::object::class_layout::is_real_instance(
                    py,
                    group,
                    crate::builtin_classes(py).base_exception_group,
                ),
                "native group class {:?}; bound runtime class {:?}; C subtype relation {}",
                CStr::from_ptr((*(*native_group).ob_type).tp_name),
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj((*native_group).ob_type.cast())
                    .map(|value| value.bits()),
                typeobj::PyType_IsSubtype(
                    (*native_group).ob_type,
                    &raw mut PyExc_BaseExceptionGroup
                ),
            );
            let split = crate::call_callable2(py, callable, group, value_class);
            assert!(
                !crate::exception_pending(py),
                "native receiver and class matcher must be admitted: {:?}",
                crate::exception_last_bits_noinc(py).map(|bits| {
                    crate::builtins::exceptions::format_exception_message(
                        py,
                        crate::obj_from_bits(bits).as_ptr().unwrap(),
                    )
                })
            );
            let parts = crate::object::seq_access::pin_tuple(
                py,
                crate::obj_from_bits(split).as_ptr().unwrap(),
            )
            .unwrap();
            assert_eq!(parts.len(), 2);
            for (&part, leaf) in parts.iter().zip([value, other]) {
                let part_children = crate::molt_get_attr_name(part, exceptions_name);
                assert!(!crate::exception_pending(py));
                let part_items = crate::object::seq_access::pin_tuple(
                    py,
                    crate::obj_from_bits(part_children).as_ptr().unwrap(),
                )
                .unwrap();
                assert_eq!(
                    &*part_items,
                    &[leaf],
                    "partition keeps exact native and managed leaf identities"
                );
                let copied_notes = crate::molt_get_attr_name(part, notes_name);
                assert_ne!(copied_notes, notes);
                assert_eq!(
                    crate::object::seq_access::len(
                        crate::obj_from_bits(copied_notes).as_ptr().unwrap()
                    ),
                    1
                );
                assert_eq!(
                    crate::object::seq_access::item(
                        crate::obj_from_bits(copied_notes).as_ptr().unwrap(),
                        0
                    ),
                    Some(note)
                );
                assert!(crate::is_truthy(
                    py,
                    crate::obj_from_bits(crate::builtins::exceptions::exception_suppress_bits(
                        crate::obj_from_bits(part).as_ptr().unwrap()
                    ))
                ));
                dec_ref_bits(py, copied_notes);
                dec_ref_bits(py, part_children);
            }
            drop(parts);

            // The same C-owned group must recurse when nested in a managed group.
            let (outer_args, outer_children) = group_args(py, &[group]);
            let outer = crate::call_callable2(
                py,
                crate::builtin_classes(py).base_exception_group,
                crate::object::seq_access::item(
                    crate::obj_from_bits(outer_args).as_ptr().unwrap(),
                    0,
                )
                .unwrap(),
                outer_children,
            );
            assert!(!crate::exception_pending(py));
            let nested = crate::call_callable2(py, callable, outer, value_class);
            assert!(
                !crate::exception_pending(py),
                "native nested groups use the shared recursive partition"
            );
            let nested_parts = crate::object::seq_access::pin_tuple(
                py,
                crate::obj_from_bits(nested).as_ptr().unwrap(),
            )
            .unwrap();
            for (&part, leaf) in nested_parts.iter().zip([value, other]) {
                let branch_items = crate::molt_get_attr_name(part, exceptions_name);
                let branch = crate::object::seq_access::item(
                    crate::obj_from_bits(branch_items).as_ptr().unwrap(),
                    0,
                )
                .unwrap();
                let leaf_items = crate::molt_get_attr_name(branch, exceptions_name);
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    crate::object::seq_access::len(
                        crate::obj_from_bits(leaf_items).as_ptr().unwrap()
                    ),
                    1
                );
                assert_eq!(
                    crate::object::seq_access::item(
                        crate::obj_from_bits(leaf_items).as_ptr().unwrap(),
                        0
                    ),
                    Some(leaf)
                );
                dec_ref_bits(py, leaf_items);
                dec_ref_bits(py, branch_items);
            }
            drop(nested_parts);

            // A visible derive override returns C-owned groups. The public
            // split path must publish metadata through the native field owners.
            let derive_name = crate::attr_name_bits_from_bytes(py, b"derive").unwrap();
            let derive =
                MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "derive_native_group",
                        derive_native_group as *const (),
                    ),
                    1,
                ))
                .bits();
            let set_result = crate::molt_set_attr_name(group, derive_name, derive);
            dec_ref_bits(py, set_result);
            let context_name = crate::attr_name_bits_from_bytes(py, b"__context__").unwrap();
            let cause_name = crate::attr_name_bits_from_bytes(py, b"__cause__").unwrap();
            let suppress_name =
                crate::attr_name_bits_from_bytes(py, b"__suppress_context__").unwrap();
            for name in [context_name, cause_name] {
                let result = crate::molt_set_attr_name(group, name, other);
                dec_ref_bits(py, result);
            }
            assert!(!crate::exception_pending(py));
            let native_parts = crate::call_callable2(py, callable, group, value_class);
            assert!(
                !crate::exception_pending(py),
                "custom derive native metadata publication"
            );
            let parts = crate::object::seq_access::pin_tuple(
                py,
                crate::obj_from_bits(native_parts).as_ptr().unwrap(),
            )
            .unwrap();
            for (&part, leaf) in parts.iter().zip([value, other]) {
                assert_eq!(
                    crate::object_type_id(crate::obj_from_bits(part).as_ptr().unwrap()),
                    crate::TYPE_ID_FOREIGN
                );
                for name in [context_name, cause_name] {
                    let observed = crate::molt_get_attr_name(part, name);
                    assert_eq!(
                        observed, other,
                        "native metadata keeps exact exception identity"
                    );
                    dec_ref_bits(py, observed);
                }
                let suppressed = crate::molt_get_attr_name(part, suppress_name);
                assert!(crate::is_truthy(py, crate::obj_from_bits(suppressed)));
                dec_ref_bits(py, suppressed);
                let copied_notes = crate::molt_get_attr_name(part, notes_name);
                assert_ne!(copied_notes, notes);
                assert_eq!(
                    crate::object::seq_access::item(
                        crate::obj_from_bits(copied_notes).as_ptr().unwrap(),
                        0
                    ),
                    Some(note)
                );
                let part_children = crate::molt_get_attr_name(part, exceptions_name);
                assert_eq!(
                    crate::object::seq_access::item(
                        crate::obj_from_bits(part_children).as_ptr().unwrap(),
                        0
                    ),
                    Some(leaf)
                );
                dec_ref_bits(py, part_children);
                dec_ref_bits(py, copied_notes);
            }
            drop(parts);
            for bits in [
                native_parts,
                derive,
                derive_name,
                context_name,
                cause_name,
                suppress_name,
            ] {
                dec_ref_bits(py, bits);
            }
            for bits in [
                nested,
                outer,
                outer_args,
                outer_children,
                split,
                callable,
                note,
                notes,
                notes_name,
                exceptions_name,
                split_name,
                group,
                args,
                children,
                other,
                value,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!crate::exception_pending(py));
        }
    });
}
