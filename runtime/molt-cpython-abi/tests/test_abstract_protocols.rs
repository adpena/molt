//! Abstract protocol regressions use canonical semantic hooks and shared fixture ownership.
#![allow(non_snake_case)]
mod support;
use molt_cpython_abi::abi_types::{Py_None, PyExc_TypeError};
use molt_cpython_abi::api::refcount::OwnedPyObject;
use molt_cpython_abi::api::{
    abstract_mapping, abstract_sequence, errors, mapping, object, sequences, strings, typeobj,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;

fn install() -> support::AbiTestThreadStateTransaction {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire_sequences(&mut hooks);
    let transaction = support::AbiTestThreadStateTransaction::new(hooks);
    support::fake_runtime::prepare_class_bindings();
    transaction
}
fn register(bits: u64) -> OwnedPyObject {
    let value = unsafe { OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)) };
    assert!(!value.as_ptr().is_null());
    value
}
fn make_str(text: &str) -> OwnedPyObject {
    register(unsafe { support::fake_runtime::alloc_str(text.as_ptr(), text.len()) })
}
fn assert_type_error() {
    assert_eq!(
        unsafe { errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()) },
        1
    );
    unsafe { errors::PyErr_Clear() };
}

// Independent oracle: CPython v3.12.15 Objects/fileobject.c and the actual
// pythonapi.PyObject_AsFileDescriptor boundary, including its C int width.
#[test]
fn file_descriptor_integer_admission_rejects_negative_and_narrowing_aliases() {
    use molt_cpython_abi::abi_types::{PyExc_OverflowError, PyExc_ValueError};
    use molt_cpython_abi::api::numbers;
    let _transaction = install();
    for (value, expected, error) in [
        (0, 0, std::ptr::null_mut()),
        (2, 2, std::ptr::null_mut()),
        (i32::MAX as i64, i32::MAX, std::ptr::null_mut()),
        (-1, -1, (&raw mut PyExc_ValueError).cast()),
        (-2, -1, (&raw mut PyExc_ValueError).cast()),
        (
            i32::MAX as i64 + 1,
            -1,
            (&raw mut PyExc_OverflowError).cast(),
        ),
        (
            i32::MIN as i64 - 1,
            -1,
            (&raw mut PyExc_OverflowError).cast(),
        ),
        ((1_i64 << 32) + 2, -1, (&raw mut PyExc_OverflowError).cast()),
    ] {
        let input = unsafe { OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(value)) };
        assert!(!input.as_ptr().is_null());
        assert_eq!(
            unsafe { object::PyObject_AsFileDescriptor(input.as_ptr()) },
            expected
        );
        if error.is_null() {
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
        } else {
            assert_eq!(
                unsafe { errors::PyErr_ExceptionMatches(error) },
                1,
                "{value}"
            );
            unsafe { errors::PyErr_Clear() };
        }
    }
    for value in [false, true] {
        let input = register(MoltObject::from_bool(value).bits());
        assert_eq!(
            unsafe { object::PyObject_AsFileDescriptor(input.as_ptr()) },
            i32::from(value)
        );
        assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    }
}

#[repr(C)]
struct DescriptorReceiver {
    base: molt_cpython_abi::abi_types::PyObject,
    method: *mut molt_cpython_abi::abi_types::PyObject,
    lookup_error: *mut molt_cpython_abi::abi_types::PyObject,
}

unsafe extern "C" fn descriptor_getattr(
    receiver: *mut molt_cpython_abi::abi_types::PyObject,
    name: *mut molt_cpython_abi::abi_types::PyObject,
) -> *mut molt_cpython_abi::abi_types::PyObject {
    use molt_cpython_abi::abi_types::PyExc_AttributeError;
    let name = unsafe { strings::PyUnicode_AsUTF8(name) };
    assert!(!name.is_null());
    assert_eq!(unsafe { std::ffi::CStr::from_ptr(name) }, c"fileno");
    let receiver = unsafe { &*receiver.cast::<DescriptorReceiver>() };
    if !receiver.lookup_error.is_null() || receiver.method.is_null() {
        let error = if receiver.lookup_error.is_null() {
            (&raw mut PyExc_AttributeError).cast()
        } else {
            receiver.lookup_error
        };
        unsafe { errors::PyErr_SetString(error, c"descriptor lookup".as_ptr()) };
        return std::ptr::null_mut();
    }
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(receiver.method) };
    receiver.method
}

unsafe extern "C" fn descriptor_return_value(
    value: *mut molt_cpython_abi::abi_types::PyObject,
    args: *mut molt_cpython_abi::abi_types::PyObject,
) -> *mut molt_cpython_abi::abi_types::PyObject {
    assert!(args.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(value) };
    value
}

unsafe extern "C" fn descriptor_index_only(
    _value: *mut molt_cpython_abi::abi_types::PyObject,
) -> *mut molt_cpython_abi::abi_types::PyObject {
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(37) }
}

#[test]
fn file_descriptor_fileno_preserves_type_width_errors_and_return_ownership() {
    use molt_cpython_abi::abi_types::{
        METH_NOARGS, PyExc_OverflowError, PyExc_RuntimeError, PyExc_ValueError, PyMethodDef,
        PyNumberMethods, PyObject, PyType_Type, PyTypeObject,
    };
    use molt_cpython_abi::api::numbers;
    let _transaction = install();
    let mut kind: PyTypeObject = unsafe { std::mem::zeroed() };
    kind.ob_base.ob_base.ob_refcnt = 1;
    kind.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    kind.tp_name = c"DescriptorReceiver".as_ptr();
    kind.tp_basicsize = std::mem::size_of::<DescriptorReceiver>() as isize;
    kind.tp_getattro = Some(descriptor_getattr);
    let mut receiver = DescriptorReceiver {
        base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut kind,
        },
        method: std::ptr::null_mut(),
        lookup_error: std::ptr::null_mut(),
    };
    let mut definition = PyMethodDef {
        ml_name: c"fileno".as_ptr(),
        ml_meth: Some(descriptor_return_value),
        ml_flags: METH_NOARGS,
        ml_doc: std::ptr::null(),
    };
    for (value, expected, error) in [
        (Some(1000), 1000, std::ptr::null_mut()),
        (Some(-2), -1, (&raw mut PyExc_ValueError).cast()),
        (
            Some((1_i64 << 32) + 2),
            -1,
            (&raw mut PyExc_OverflowError).cast(),
        ),
        (None, -1, (&raw mut PyExc_TypeError).cast()),
    ] {
        let value = if let Some(value) = value {
            unsafe { OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(value)) }
        } else {
            register(MoltObject::none().bits())
        };
        let method = unsafe {
            OwnedPyObject::from_owned(object::PyCFunction_New(&raw mut definition, value.as_ptr()))
        };
        assert!(!method.as_ptr().is_null());
        receiver.method = method.as_ptr();
        let before = unsafe { (*value.as_ptr()).ob_refcnt };
        assert_eq!(
            unsafe { object::PyObject_AsFileDescriptor(&raw mut receiver.base) },
            expected
        );
        assert_eq!(unsafe { (*value.as_ptr()).ob_refcnt }, before);
        if error.is_null() {
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
        } else {
            assert_eq!(unsafe { errors::PyErr_ExceptionMatches(error) }, 1);
            unsafe { errors::PyErr_Clear() };
        }
        receiver.method = std::ptr::null_mut();
    }
    // A fileno result may not use __index__, even when that protocol would
    // produce an in-range descriptor. PyLong_AsLong alone over-admits this.
    let mut index_methods: PyNumberMethods = unsafe { std::mem::zeroed() };
    index_methods.nb_index = descriptor_index_only as *mut std::ffi::c_void;
    let mut index_kind: PyTypeObject = unsafe { std::mem::zeroed() };
    index_kind.ob_base.ob_base.ob_refcnt = 1;
    index_kind.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    index_kind.tp_name = c"IndexOnly".as_ptr();
    index_kind.tp_as_number = (&raw mut index_methods).cast();
    let mut index_only = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut index_kind,
    };
    {
        let method = unsafe {
            OwnedPyObject::from_owned(object::PyCFunction_New(
                &raw mut definition,
                &raw mut index_only,
            ))
        };
        assert!(!method.as_ptr().is_null());
        receiver.method = method.as_ptr();
        assert_eq!(
            unsafe { object::PyObject_AsFileDescriptor(&raw mut receiver.base) },
            -1
        );
        assert_type_error();
        receiver.method = std::ptr::null_mut();
    }
    assert_eq!(index_only.ob_refcnt, 1);
    for (lookup_error, expected) in [
        (std::ptr::null_mut(), (&raw mut PyExc_TypeError).cast()),
        (
            (&raw mut PyExc_RuntimeError).cast(),
            (&raw mut PyExc_RuntimeError).cast(),
        ),
    ] {
        receiver.lookup_error = lookup_error;
        assert_eq!(
            unsafe { object::PyObject_AsFileDescriptor(&raw mut receiver.base) },
            -1
        );
        assert_eq!(unsafe { errors::PyErr_ExceptionMatches(expected) }, 1);
        unsafe { errors::PyErr_Clear() };
    }
}

// Contains/count/index use the real runtime in cpython_abi_hooks/inquiry_tests.rs,
// through both compiled C header transports.
#[test]
fn tuple_and_list_of_non_iterable_raise_typeerror_not_empty() {
    let _transaction = install();
    let n = register(MoltObject::from_int(42).bits());
    let t = unsafe { abstract_sequence::PySequence_Tuple(n.as_ptr()) };
    assert!(
        t.is_null(),
        "PySequence_Tuple(non-iterable) must be NULL, not an empty tuple"
    );
    assert_type_error();
    let l = unsafe { abstract_sequence::PySequence_List(n.as_ptr()) };
    assert!(
        l.is_null(),
        "PySequence_List(non-iterable) must be NULL, not an empty list"
    );
    assert_type_error();
}

#[test]
fn str_materializes_into_code_point_tuple() {
    let _transaction = install();
    // Keep the original ASCII witness and distinguish code points from bytes.
    for (text, expected) in [("ab", vec!["a", "b"]), ("aé😀", vec!["a", "é", "😀"])] {
        let source = make_str(text);
        let source_bits = GLOBAL_BRIDGE
            .molt_handle_for_pyobj(source.as_ptr())
            .unwrap()
            .bits();
        let tuple = unsafe {
            OwnedPyObject::from_owned(abstract_sequence::PySequence_Tuple(source.as_ptr()))
        };
        assert!(
            !tuple.as_ptr().is_null(),
            "PySequence_Tuple(str) must materialize the code points"
        );
        assert_eq!(
            unsafe { abstract_sequence::PySequence_Fast_GET_SIZE(tuple.as_ptr()) },
            expected.len() as isize
        );
        // The sibling list constructor consumes the same iterator with a length hint.
        let list = unsafe {
            OwnedPyObject::from_owned(abstract_sequence::PySequence_List(source.as_ptr()))
        };
        assert!(!list.as_ptr().is_null());
        assert_eq!(
            unsafe { sequences::PyList_Size(list.as_ptr()) },
            expected.len() as isize
        );
        drop(source);
        assert!(
            !support::fake_runtime::contains(source_bits),
            "completed iterators must release their source"
        );
        for (index, expected) in expected.into_iter().enumerate() {
            for item in [
                unsafe { sequences::PyTuple_GetItem(tuple.as_ptr(), index as isize) },
                unsafe { sequences::PyList_GetItem(list.as_ptr(), index as isize) },
            ] {
                assert!(!item.is_null());
                let mut length = 0;
                let data = unsafe { strings::PyUnicode_AsUTF8AndSize(item, &mut length) };
                assert!(!data.is_null());
                assert_eq!(
                    unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) },
                    expected.as_bytes()
                );
            }
        }
        assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    }
}

#[test]
fn sequence_setitem_on_tuple_raises_typeerror() {
    let _transaction = install();
    let tuple = unsafe { OwnedPyObject::from_owned(sequences::PyTuple_New(1)) };
    assert!(!tuple.as_ptr().is_null());
    // Fill the initial slot explicitly: Missing construction state is not None.
    assert_eq!(
        unsafe {
            sequences::PyTuple_SetItem(tuple.as_ptr(), 0, object::Py_NewRef(&raw mut Py_None))
        },
        0
    );
    let value = make_str("x");
    let rc = unsafe { abstract_sequence::PySequence_SetItem(tuple.as_ptr(), 0, value.as_ptr()) };
    assert_eq!(rc, -1, "immutable tuple must reject item assignment");
    assert_type_error();
    assert_eq!(
        unsafe { sequences::PyTuple_GetItem(tuple.as_ptr(), 0) },
        &raw mut Py_None,
        "the pre-fix silent tuple mutation must be locked out"
    );
}

#[test]
fn sequence_size_of_dict_raises_typeerror() {
    let _transaction = install();
    let dict = unsafe { OwnedPyObject::from_owned(mapping::PyDict_New()) };
    assert!(!dict.as_ptr().is_null());
    for key in ["first", "second", "third"] {
        let key = make_str(key);
        assert_eq!(
            unsafe { mapping::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), &raw mut Py_None) },
            0
        );
    }
    // Dict length belongs to the mapping protocol, not the sequence protocol.
    assert_eq!(
        unsafe { abstract_sequence::PySequence_Size(dict.as_ptr()) },
        -1
    );
    assert_type_error();
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Size(dict.as_ptr()) },
        3
    );
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn mapping_check_accepts_subscriptable_natives() {
    let _transaction = install();
    let list = unsafe { OwnedPyObject::from_owned(sequences::PyList_New(0)) };
    assert!(!list.as_ptr().is_null());
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Check(list.as_ptr()) },
        1
    );
    let string = make_str("m");
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Check(string.as_ptr()) },
        1
    );
    let tuple = unsafe { OwnedPyObject::from_owned(sequences::PyTuple_New(0)) };
    assert!(!tuple.as_ptr().is_null());
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Check(tuple.as_ptr()) },
        1
    );
    let dict = unsafe { OwnedPyObject::from_owned(mapping::PyDict_New()) };
    assert!(!dict.as_ptr().is_null());
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Check(dict.as_ptr()) },
        1
    );
    let integer = register(MoltObject::from_int(42).bits());
    assert_eq!(
        unsafe { abstract_mapping::PyMapping_Check(integer.as_ptr()) },
        0
    );
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn mapping_check_uses_runtime_semantics_instead_of_physical_c_slots() {
    // The process owns one immutable hook table. Declare the receiver's
    // capability in the shared fixture owner instead of trying to replace
    // hooks after another test has already installed them.
    let _transaction = install();
    let receiver = register(support::fake_runtime::subscriptable_handle());
    unsafe {
        // Deliberately differ from the physical shell. This fixture proves
        // forwarding; the real-runtime matrix owns Python protocol semantics.
        let class = (*receiver.as_ptr()).ob_type;
        assert!(!class.is_null());
        assert!(
            typeobj::PyType_GetSlot(class, molt_cpython_abi::type_slots::Py_mp_subscript,)
                .is_null()
        );
        assert_eq!(abstract_mapping::PyMapping_Check(receiver.as_ptr()), 1);
        assert!(errors::PyErr_Occurred().is_null());
    }
}
