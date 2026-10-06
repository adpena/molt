//! object owns real C slots. Python namespace resolution preserves these exact
//! functions, including the asymmetric CPython object constructor argument rule.
use super::*;
use crate::abi_types::*;
use crate::api::{abstract_sequence, mapping, object, refcount::OwnedPyObject, sequences, strings};

unsafe fn excess_args(
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> Result<bool, crate::ErrorIndicatorSet> {
    unsafe {
        let positional = sequences::PyTuple_Size(args);
        if positional < 0 {
            return Err(crate::ErrorIndicatorSet);
        }
        let keywords = if kwds.is_null() {
            0
        } else {
            mapping::PyDict_Size(kwds)
        };
        if keywords < 0 {
            return Err(crate::ErrorIndicatorSet);
        }
        Ok(positional != 0 || keywords != 0)
    }
}

pub(crate) unsafe extern "C" fn object_init(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> c_int {
    unsafe {
        let tp = crate::bridge::semantic_type(receiver);
        if tp.is_null() {
            return -1;
        }
        match excess_args(args, kwds) {
            Err(crate::ErrorIndicatorSet) => return -1,
            Ok(false) => return 0,
            Ok(true) => {}
        }
        if (*tp).tp_init.map(|f| f as *const ()) != Some(object_init as *const ()) {
            descriptors::type_error(
                "object.__init__() takes exactly one argument (the instance to initialize)",
            );
            return -1;
        }
        if (*tp).tp_new.map(|f| f as *const ()) == Some(object_new as *const ()) {
            descriptors::type_error(&format!(
                "{}.__init__() takes exactly one argument (the instance to initialize)",
                object_type_name(receiver)
            ));
            return -1;
        }
        0
    }
}

unsafe fn reject_abstract(tp: *mut PyTypeObject) -> bool {
    unsafe {
        if (*tp).tp_flags & Py_TPFLAGS_IS_ABSTRACT == 0 {
            return false;
        }
        let methods = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            tp.cast(),
            c"__abstractmethods__".as_ptr(),
        ));
        if methods.as_ptr().is_null() {
            return true;
        }
        let sorted =
            OwnedPyObject::from_owned(abstract_sequence::PySequence_List(methods.as_ptr()));
        if sorted.as_ptr().is_null() || sequences::PyList_Sort(sorted.as_ptr()) < 0 {
            return true;
        }
        let count = sequences::PyList_Size(sorted.as_ptr());
        if count < 0 {
            return true;
        }
        let separator = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"', '".as_ptr()));
        if separator.as_ptr().is_null() {
            return true;
        }
        let joined =
            OwnedPyObject::from_owned(strings::PyUnicode_Join(separator.as_ptr(), sorted.as_ptr()));
        if joined.as_ptr().is_null() {
            return true;
        }
        let Some(bytes) = strings::unicode_bytes(joined.as_ptr()) else {
            return true;
        };
        let name = std::ffi::CStr::from_ptr((*tp).tp_name).to_string_lossy();
        descriptors::type_error(&format!(
            "Can't instantiate abstract class {name} without an implementation for abstract method{} '{}'",
            if count > 1 { "s" } else { "" },
            String::from_utf8_lossy(bytes)
        ));
        true
    }
}

pub(crate) unsafe extern "C" fn object_new(
    tp: *mut PyTypeObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let _owner = OwnedPyObject::from_borrowed(tp.cast());
        match excess_args(args, kwds) {
            Err(crate::ErrorIndicatorSet) => return ptr::null_mut(),
            Ok(false) => {}
            Ok(true) => {
                if (*tp).tp_new.map(|f| f as *const ()) != Some(object_new as *const ()) {
                    return descriptors::type_error(
                        "object.__new__() takes exactly one argument (the type to instantiate)",
                    );
                }
                if (*tp).tp_init.map(|f| f as *const ()) == Some(object_init as *const ()) {
                    return descriptors::type_error(&format!(
                        "{}() takes no arguments",
                        std::ffi::CStr::from_ptr((*tp).tp_name).to_string_lossy()
                    ));
                }
            }
        }
        if reject_abstract(tp) {
            return ptr::null_mut();
        }
        // The allocator owns physical initialization, including zeroed native
        // dictionary storage. Generic attributes initialize that dictionary
        // through the existing layout/storage authority on first use.
        let Some(allocate) = (*tp).tp_alloc else {
            descriptors::system_error(c"object type has no allocator");
            return ptr::null_mut();
        };
        allocate(tp, 0)
    }
}

pub(crate) unsafe extern "C" fn object_repr(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        let name = object_type_name(receiver);
        let rendered = format!("<{name} object at {receiver:p}>");
        match std::ffi::CString::new(rendered) {
            Ok(text) => strings::PyUnicode_FromString(text.as_ptr()),
            Err(_) => {
                descriptors::system_error(c"object type name contains NUL");
                ptr::null_mut()
            }
        }
    }
}

pub(crate) unsafe extern "C" fn object_str(receiver: *mut PyObject) -> *mut PyObject {
    unsafe { PyObject_Repr(receiver) }
}

pub(crate) unsafe extern "C" fn object_richcompare(
    receiver: *mut PyObject,
    other: *mut PyObject,
    operation: c_int,
) -> *mut PyObject {
    unsafe {
        match operation {
            CMP_EQ if receiver == other => cmp_bool_result(true),
            CMP_NE => {
                let tp = crate::bridge::semantic_type(receiver);
                if tp.is_null() {
                    return ptr::null_mut();
                }
                let Some(compare) = (*tp).tp_richcompare else {
                    return richcmp_not_implemented();
                };
                let result = OwnedPyObject::from_owned(compare(receiver, other, CMP_EQ));
                if result.as_ptr().is_null() {
                    return ptr::null_mut();
                }
                if is_not_implemented(result.as_ptr()) {
                    return richcmp_not_implemented();
                }
                match object::PyObject_IsTrue(result.as_ptr()) {
                    -1 => ptr::null_mut(),
                    truth => cmp_bool_result(truth == 0),
                }
            }
            _ => richcmp_not_implemented(),
        }
    }
}

/// CPython's native __new__ callable retains a declaring type and invokes its
/// physical constructor after checking subtype and static-base safety.
unsafe extern "C" fn tp_new_wrapper(
    owner: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if owner.is_null() || PyType_Check(owner) == 0 {
            descriptors::system_error(c"__new__() called with non-type 'self'");
            return ptr::null_mut();
        }
        let count = sequences::PyTuple_Size(args);
        if count < 0 {
            return ptr::null_mut();
        }
        if count < 1 {
            return descriptors::type_error("__new__(): not enough arguments");
        }
        let subtype = sequences::PyTuple_GetItem(args, 0);
        if subtype.is_null() {
            return ptr::null_mut();
        }
        if PyType_Check(subtype) == 0 {
            return descriptors::type_error("__new__(X): X is not a type object");
        }
        let tp = owner.cast::<PyTypeObject>();
        let subtype = subtype.cast::<PyTypeObject>();
        if PyType_IsSubtype(subtype, tp) == 0 {
            return descriptors::type_error("__new__(X): X is not a subtype of the declaring type");
        }
        let constructor = (*tp).tp_new.map(|f| f as *const () as *mut c_void);
        let mut staticbase = subtype;
        while !staticbase.is_null()
            && (*staticbase).tp_new.map(|f| f as *const () as *mut c_void)
                == Some(native_slot_dispatch::dispatcher(SlotWrapper::Direct(
                    DirectSlot::New,
                )))
        {
            staticbase = (*staticbase).tp_base;
        }
        if !staticbase.is_null()
            && (*staticbase).tp_new.map(|f| f as *const () as *mut c_void) != constructor
        {
            return descriptors::type_error("__new__(X) is not safe; use X.__new__()");
        }
        let tail = OwnedPyObject::from_owned(sequences::PyTuple_GetSlice(args, 1, count));
        if tail.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let Some(new) = (*tp).tp_new else {
            return descriptors::type_error("type has no constructor");
        };
        new(subtype, tail.as_ptr(), kwds)
    }
}

pub(super) unsafe fn is_new_wrapper(descriptor: *mut PyObject) -> bool {
    unsafe {
        if (*descriptor).ob_type != &raw mut PyCFunction_Type {
            return false;
        }
        let method = (*descriptor.cast::<PyCFunctionObject>()).m_ml;
        !method.is_null()
            && (*method).ml_meth.map(|f| f as *const ()) == Some(tp_new_wrapper as *const ())
    }
}

static mut NEW_METHOD: PyMethodDef = PyMethodDef {
    ml_name: c"__new__".as_ptr(),
    ml_meth: Some(unsafe {
        std::mem::transmute::<
            unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
            PyCFunction,
        >(tp_new_wrapper)
    }),
    ml_flags: METH_VARARGS | METH_KEYWORDS,
    ml_doc: c"__new__($type, *args, **kwargs)\n--\n\nCreate and return a new object.".as_ptr(),
};

pub(super) unsafe fn add_new_wrapper(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        if (*tp).tp_new.is_none() {
            return 0;
        }
        let existing = mapping::_PyDict_GetItemStringWithError((*tp).tp_dict, c"__new__".as_ptr());
        if descriptors::pending() {
            return -1;
        }
        if !existing.is_null() {
            return 0;
        }
        let wrapper = OwnedPyObject::from_owned(object::PyCFunction_NewEx(
            &raw mut NEW_METHOD,
            tp.cast(),
            ptr::null_mut(),
        ));
        if wrapper.as_ptr().is_null() {
            return -1;
        }
        mapping::PyDict_SetItemString((*tp).tp_dict, c"__new__".as_ptr(), wrapper.as_ptr())
    }
}
