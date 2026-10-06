//! Native method declarations bind through the descriptor protocol. Unbound
//! calls and bound CFunctions share the C calling-convention authority.

use super::descriptors::{self, Pins, pending, system_error, type_error};
use crate::abi_types::*;
use crate::api::callback::CallbackOperands;
use crate::api::cfunction::CFunctionConvention;
use crate::api::{errors, object, sequences, strings};
use std::ptr;

unsafe fn method_parts(definition: *mut PyMethodDef) -> Option<(CFunctionConvention, PyCFunction)> {
    let Some(method) = (unsafe { definition.as_ref() }) else {
        unsafe { errors::PyErr_BadInternalCall() };
        return None;
    };
    match (
        CFunctionConvention::from_flags(method.ml_flags),
        method.ml_meth,
    ) {
        (Some(convention), Some(target)) => Some((convention, target)),
        _ => {
            unsafe { system_error(c"invalid native method descriptor definition") };
            None
        }
    }
}

unsafe fn descriptor_name(descr: *mut PyObject) -> String {
    unsafe { strings::unicode_bytes((*descr.cast::<PyDescrObject>()).d_name) }
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_else(|| "?".to_owned())
}

unsafe fn new_descriptor(
    kind: *mut PyTypeObject,
    owner: *mut PyTypeObject,
    definition: *mut PyMethodDef,
) -> *mut PyObject {
    if unsafe { method_parts(definition) }.is_none() {
        return ptr::null_mut();
    }
    let descr = unsafe { descriptors::allocate(kind, owner, (*definition).ml_name) }
        .cast::<PyMethodDescrObject>();
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*descr).d_method = definition;
        (*descr).vectorcall = if kind == &raw mut PyMethodDescr_Type {
            Some(method_vectorcall)
        } else {
            None
        };
        descriptors::publish(descr.cast())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewMethod(
    owner: *mut PyTypeObject,
    definition: *mut PyMethodDef,
) -> *mut PyObject {
    unsafe { new_descriptor(&raw mut PyMethodDescr_Type, owner, definition) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewClassMethod(
    owner: *mut PyTypeObject,
    definition: *mut PyMethodDef,
) -> *mut PyObject {
    unsafe { new_descriptor(&raw mut PyClassMethodDescr_Type, owner, definition) }
}

/// Construct through the canonical runtime class binding. The runtime owns
/// wrapper storage, metadata and descriptor semantics; the C API never
/// resolves a mutable builtins name or introduces another wrapper object.
unsafe fn new_wrapper(class: *mut PyTypeObject, callable: *mut PyObject) -> *mut PyObject {
    if callable.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let class = class.cast();
    if !unsafe { object::runtime_call_authority(class, true) } {
        unsafe { system_error(c"descriptor wrapper runtime class is unavailable") };
        return ptr::null_mut();
    }
    unsafe { object::PyObject_CallOneArg(class, callable) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyStaticMethod_New(callable: *mut PyObject) -> *mut PyObject {
    unsafe { new_wrapper(&raw mut PyStaticMethod_Type, callable) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyClassMethod_New(callable: *mut PyObject) -> *mut PyObject {
    unsafe { new_wrapper(&raw mut PyClassMethod_Type, callable) }
}

unsafe extern "C" fn method_get(
    descr: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    if receiver.is_null() {
        return unsafe { descriptors::new_reference(descr) };
    }
    let _pins = unsafe { Pins::new([descr, receiver, owner]) };
    if !unsafe { descriptors::receiver(descr, receiver) } {
        return ptr::null_mut();
    }
    let method = descr.cast::<PyMethodDescrObject>();
    let Some((convention, _)) = (unsafe { method_parts((*method).d_method) }) else {
        return ptr::null_mut();
    };
    let defining_class = if convention == CFunctionConvention::Method {
        if unsafe { super::PyType_Check(owner) } == 0 {
            if !pending() {
                unsafe {
                    type_error(&format!(
                        "descriptor '{}' needs a type as arg 2",
                        descriptor_name(descr)
                    ))
                };
            }
            return ptr::null_mut();
        }
        unsafe { (*method).d_common.d_type }
    } else {
        ptr::null_mut()
    };
    unsafe {
        object::PyCMethod_New(
            (*method).d_method,
            receiver,
            ptr::null_mut(),
            defining_class,
        )
    }
}

unsafe extern "C" fn classmethod_get(
    descr: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    let _pins = unsafe { Pins::new([descr, receiver, owner]) };
    // An explicit owner wins even if the supplied object is unrelated.
    let candidate = if owner.is_null() {
        if receiver.is_null() {
            return unsafe {
                type_error("classmethod descriptor needs either an object or a type")
            };
        }
        unsafe { crate::bridge::semantic_type(receiver) }.cast::<PyObject>()
    } else {
        owner
    };
    if candidate.is_null() || unsafe { super::PyType_Check(candidate) } == 0 {
        if !pending() {
            unsafe { type_error("classmethod descriptor needs a type as arg 2") };
        }
        return ptr::null_mut();
    }
    let _candidate = unsafe { Pins::new([candidate]) };
    let method = descr.cast::<PyMethodDescrObject>();
    let declaring_class = unsafe { (*method).d_common.d_type };
    if declaring_class.is_null() {
        unsafe { system_error(c"descriptor owner has been cleared") };
        return ptr::null_mut();
    }
    if unsafe { super::PyType_IsSubtype(candidate.cast(), declaring_class) } == 0 {
        if !pending() {
            unsafe {
                type_error("classmethod descriptor requires a subtype of its declaring type")
            };
        }
        return ptr::null_mut();
    }
    let Some((convention, _)) = (unsafe { method_parts((*method).d_method) }) else {
        return ptr::null_mut();
    };
    let defining_class = if convention == CFunctionConvention::Method {
        declaring_class
    } else {
        ptr::null_mut()
    };
    unsafe {
        object::PyCMethod_New(
            (*method).d_method,
            candidate,
            ptr::null_mut(),
            defining_class,
        )
    }
}

unsafe extern "C" fn method_vectorcall(
    descr: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    let Some(values) = (unsafe { object::vectorcall_argument_span(args, nargsf, kwnames) }) else {
        return ptr::null_mut();
    };
    let positional = object::vectorcall_nargs(nargsf) as usize;
    if positional == 0 {
        return unsafe {
            type_error(&format!(
                "unbound method {}() needs an argument",
                descriptor_name(descr)
            ))
        };
    }
    let method = descr.cast::<PyMethodDescrObject>();
    let owner = unsafe { (*method).d_common.d_type };
    let Some(operands) =
        (unsafe { CallbackOperands::from_vector([descr, owner.cast(), kwnames], values) })
    else {
        return ptr::null_mut();
    };
    if !unsafe { descriptors::receiver(descr, values[0]) } {
        return ptr::null_mut();
    }
    let Some((convention, target)) = (unsafe { method_parts((*method).d_method) }) else {
        return ptr::null_mut();
    };
    let defining_class = if convention == CFunctionConvention::Method {
        owner
    } else {
        ptr::null_mut()
    };
    let result = unsafe {
        convention.invoke(
            target as *const (),
            values[0],
            defining_class,
            crate::api::cfunction::VectorcallArguments {
                values: &values[1..],
                positional_count: positional - 1,
                kwnames,
            },
            || descriptor_name(descr),
        )
    };
    unsafe { operands.complete_result(result, "native method descriptor") }
}

unsafe extern "C" fn classmethod_call(
    descr: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let _pins = unsafe { Pins::new([descr, args, kwargs]) };
    let argc = unsafe { sequences::PyTuple_Size(args) };
    if argc < 0 {
        return ptr::null_mut();
    }
    if argc == 0 {
        return unsafe { type_error("classmethod descriptor needs an argument") };
    }
    let owner = unsafe { sequences::PyTuple_GetItem(args, 0) };
    if owner.is_null() {
        return ptr::null_mut();
    }
    let bound = unsafe { classmethod_get(descr, ptr::null_mut(), owner) };
    if bound.is_null() {
        return ptr::null_mut();
    }
    let tail = unsafe { sequences::PyTuple_GetSlice(args, 1, argc) };
    let result = if tail.is_null() {
        ptr::null_mut()
    } else {
        unsafe { object::PyObject_Call(bound, tail, kwargs) }
    };
    unsafe { errors::release_preserving_error(&[tail, bound]) };
    result
}

pub(crate) fn completes_vectorcall(call: PyVectorcallFunc) -> bool {
    std::ptr::fn_addr_eq(call, method_vectorcall as PyVectorcallFunc)
}

pub(crate) fn completes_call_operands(
    call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
) -> bool {
    type Call = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;
    std::ptr::fn_addr_eq(call, classmethod_call as Call)
}

pub(super) unsafe fn init() {
    unsafe {
        descriptors::init_callable_type(
            &raw mut PyMethodDescr_Type,
            std::mem::size_of::<PyMethodDescrObject>(),
        );
        PyMethodDescr_Type.tp_flags |= Py_TPFLAGS_HAVE_VECTORCALL | Py_TPFLAGS_METHOD_DESCRIPTOR;
        PyMethodDescr_Type.tp_vectorcall_offset =
            std::mem::offset_of!(PyMethodDescrObject, vectorcall) as Py_ssize_t;
        PyMethodDescr_Type.tp_call = Some(object::PyVectorcall_Call);
        PyMethodDescr_Type.tp_descr_get = Some(method_get);

        descriptors::init_callable_type(
            &raw mut PyClassMethodDescr_Type,
            std::mem::size_of::<PyMethodDescrObject>(),
        );
        PyClassMethodDescr_Type.tp_call = Some(classmethod_call);
        PyClassMethodDescr_Type.tp_descr_get = Some(classmethod_get);
    }
}
