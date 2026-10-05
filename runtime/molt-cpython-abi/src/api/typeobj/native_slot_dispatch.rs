//! Typed Python dispatch targets for the canonical native slot declarations.
//! The slot id selects an ABI; its Python names come from SLOT_WRAPPER_DEFS.
use super::*;
use crate::abi_types::*;
use crate::api::{
    abstract_number, errors, memory, numbers, object, refcount::OwnedPyObject, sequences, strings,
};

#[path = "native_slot_buffer.rs"]
mod buffer;

pub(super) const fn dispatcher(slot: SlotWrapper) -> *mut c_void {
    macro_rules! pointer {
        ($function:expr) => {
            $function as *const () as *mut c_void
        };
    }
    match slot {
        SlotWrapper::Direct(DirectSlot::Repr) => pointer!(repr),
        SlotWrapper::Direct(DirectSlot::Str) => pointer!(unary::<{ ts::Py_tp_str }>),
        SlotWrapper::Direct(DirectSlot::Hash) => pointer!(hash),
        SlotWrapper::Direct(DirectSlot::Call) => pointer!(call),
        SlotWrapper::Direct(DirectSlot::GetAttr) => pointer!(getattribute),
        SlotWrapper::Direct(DirectSlot::SetAttr) => pointer!(setattribute),
        SlotWrapper::Direct(DirectSlot::RichCompare) => pointer!(compare),
        SlotWrapper::Direct(DirectSlot::Iter) => pointer!(iter),
        SlotWrapper::Direct(DirectSlot::IterNext) => pointer!(unary::<{ ts::Py_tp_iternext }>),
        SlotWrapper::Direct(DirectSlot::DescrGet) => pointer!(descriptor_get),
        SlotWrapper::Direct(DirectSlot::DescrSet) => pointer!(descriptor_set),
        SlotWrapper::Direct(DirectSlot::Init) => pointer!(init),
        SlotWrapper::Direct(DirectSlot::New) => pointer!(new),
        SlotWrapper::Direct(DirectSlot::Finalize) => pointer!(finalize),
        SlotWrapper::Mapping(MappingSlot::Length) => pointer!(length::<{ ts::Py_mp_length }>),
        SlotWrapper::Mapping(MappingSlot::Subscript) => pointer!(subscript),
        SlotWrapper::Mapping(MappingSlot::AssSubscript) => pointer!(setitem),
        SlotWrapper::Sequence(SequenceSlot::Length) => pointer!(length::<{ ts::Py_sq_length }>),
        SlotWrapper::Sequence(SequenceSlot::Item) => pointer!(getitem_index),
        SlotWrapper::Sequence(SequenceSlot::AssItem) => pointer!(setitem_index),
        SlotWrapper::Sequence(SequenceSlot::Contains) => pointer!(contains),
        // CPython leaves these NULL for Python methods: abstract sequence APIs
        // fall through to numeric slots, preserving NotImplemented semantics.
        SlotWrapper::Sequence(_) => ptr::null_mut(),
        SlotWrapper::Async(AsyncSlot::Await) => pointer!(async_unary::<{ ts::Py_am_await }>),
        SlotWrapper::Async(AsyncSlot::Iter) => pointer!(async_unary::<{ ts::Py_am_aiter }>),
        SlotWrapper::Async(AsyncSlot::Next) => pointer!(async_unary::<{ ts::Py_am_anext }>),
        SlotWrapper::Buffer(BufferSlot::Get) => pointer!(buffer::get),
        SlotWrapper::Buffer(BufferSlot::Release) => pointer!(buffer::release),
        SlotWrapper::Number(NumberSlot::Negative) => pointer!(unary::<{ ts::Py_nb_negative }>),
        SlotWrapper::Number(NumberSlot::Positive) => pointer!(unary::<{ ts::Py_nb_positive }>),
        SlotWrapper::Number(NumberSlot::Absolute) => pointer!(unary::<{ ts::Py_nb_absolute }>),
        SlotWrapper::Number(NumberSlot::Invert) => pointer!(unary::<{ ts::Py_nb_invert }>),
        SlotWrapper::Number(NumberSlot::Int) => pointer!(unary::<{ ts::Py_nb_int }>),
        SlotWrapper::Number(NumberSlot::Float) => pointer!(unary::<{ ts::Py_nb_float }>),
        SlotWrapper::Number(NumberSlot::Index) => pointer!(unary::<{ ts::Py_nb_index }>),
        SlotWrapper::Number(NumberSlot::Bool) => pointer!(truth),
        SlotWrapper::Number(NumberSlot::Power) => pointer!(power),
        SlotWrapper::Number(NumberSlot::InPlacePower) => pointer!(inplace_power),
        SlotWrapper::Number(NumberSlot::Add) => pointer!(binary::<{ ts::Py_nb_add }>),
        SlotWrapper::Number(NumberSlot::Subtract) => pointer!(binary::<{ ts::Py_nb_subtract }>),
        SlotWrapper::Number(NumberSlot::Multiply) => pointer!(binary::<{ ts::Py_nb_multiply }>),
        SlotWrapper::Number(NumberSlot::Remainder) => pointer!(binary::<{ ts::Py_nb_remainder }>),
        SlotWrapper::Number(NumberSlot::Divmod) => pointer!(binary::<{ ts::Py_nb_divmod }>),
        SlotWrapper::Number(NumberSlot::LShift) => pointer!(binary::<{ ts::Py_nb_lshift }>),
        SlotWrapper::Number(NumberSlot::RShift) => pointer!(binary::<{ ts::Py_nb_rshift }>),
        SlotWrapper::Number(NumberSlot::And) => pointer!(binary::<{ ts::Py_nb_and }>),
        SlotWrapper::Number(NumberSlot::Xor) => pointer!(binary::<{ ts::Py_nb_xor }>),
        SlotWrapper::Number(NumberSlot::Or) => pointer!(binary::<{ ts::Py_nb_or }>),
        SlotWrapper::Number(NumberSlot::FloorDivide) => {
            pointer!(binary::<{ ts::Py_nb_floor_divide }>)
        }
        SlotWrapper::Number(NumberSlot::TrueDivide) => {
            pointer!(binary::<{ ts::Py_nb_true_divide }>)
        }
        SlotWrapper::Number(NumberSlot::MatrixMultiply) => {
            pointer!(binary::<{ ts::Py_nb_matrix_multiply }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceAdd) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_add }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceSubtract) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_subtract }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceMultiply) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_multiply }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceRemainder) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_remainder }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceLShift) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_lshift }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceRShift) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_rshift }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceAnd) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_and }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceXor) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_xor }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceOr) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_or }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceFloorDivide) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_floor_divide }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceTrueDivide) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_true_divide }>)
        }
        SlotWrapper::Number(NumberSlot::InPlaceMatrixMultiply) => {
            pointer!(single_binary::<{ ts::Py_nb_inplace_matrix_multiply }>)
        }
        _ => ptr::null_mut(),
    }
}

unsafe fn name<const SLOT: c_int>(variant: usize) -> OwnedPyObject {
    let slot = stable_slot_wrapper(SLOT).expect("declared slot id");
    let declaration = SLOT_WRAPPER_DEFS
        .iter()
        .filter(|row| row.slot == slot)
        .nth(variant)
        .expect("declared slot variant");
    unsafe { OwnedPyObject::from_owned(strings::PyUnicode_FromString(declaration.base.name)) }
}

unsafe fn special<const SLOT: c_int>(
    receiver: *mut PyObject,
    variant: usize,
) -> Result<Option<OwnedPyObject>, ()> {
    let key = unsafe { name::<SLOT>(variant) };
    if key.as_ptr().is_null() {
        return Err(());
    }
    unsafe { object::lookup_type_special(receiver, key.as_ptr()) }
}

unsafe fn invoke<const SLOT: c_int>(
    receiver: *mut PyObject,
    variant: usize,
    arguments: &[*mut PyObject],
    optional: bool,
) -> *mut PyObject {
    match unsafe { special::<SLOT>(receiver, variant) } {
        Err(()) => ptr::null_mut(),
        Ok(None) if optional => unsafe { object::Py_NewRef(&raw mut Py_NotImplementedSentinel) },
        Ok(None) => {
            let key = unsafe { name::<SLOT>(variant) };
            if key.as_ptr().is_null() {
                return ptr::null_mut();
            }
            unsafe {
                errors::PyErr_SetObject((&raw mut PyExc_AttributeError).cast(), key.as_ptr())
            };
            ptr::null_mut()
        }
        Ok(Some(method)) => unsafe {
            object::PyObject_VectorcallDict(
                method.as_ptr(),
                arguments.as_ptr().cast_mut(),
                arguments.len(),
                ptr::null_mut(),
            )
        },
    }
}

unsafe extern "C" fn repr(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        match special::<{ ts::Py_tp_repr }>(receiver, 0) {
            Ok(Some(method)) => object::PyObject_CallNoArgs(method.as_ptr()),
            Err(()) | Ok(None) => {
                errors::PyErr_Clear();
                errors::PyUnicode_FromFormat(
                    c"<%s object at %p>".as_ptr(),
                    (*crate::bridge::semantic_type(receiver)).tp_name,
                    receiver,
                )
            }
        }
    }
}
unsafe extern "C" fn async_unary<const SLOT: c_int>(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        match special::<SLOT>(receiver, 0) {
            Ok(Some(method)) => object::PyObject_CallNoArgs(method.as_ptr()),
            Err(()) | Ok(None) => {
                let slot = stable_slot_wrapper(SLOT).expect("declared async slot");
                let declaration = SLOT_WRAPPER_DEFS
                    .iter()
                    .find(|row| row.slot == slot)
                    .unwrap();
                errors::PyErr_Format(
                    (&raw mut PyExc_AttributeError).cast(),
                    c"object %.50s does not have %s method".as_ptr(),
                    (*crate::bridge::semantic_type(receiver)).tp_name,
                    declaration.base.name,
                );
                ptr::null_mut()
            }
        }
    }
}
unsafe extern "C" fn unary<const SLOT: c_int>(receiver: *mut PyObject) -> *mut PyObject {
    unsafe { invoke::<SLOT>(receiver, 0, &[], false) }
}
unsafe extern "C" fn single_binary<const SLOT: c_int>(
    receiver: *mut PyObject,
    argument: *mut PyObject,
) -> *mut PyObject {
    unsafe { invoke::<SLOT>(receiver, 0, &[argument], true) }
}

unsafe extern "C" fn subscript(receiver: *mut PyObject, argument: *mut PyObject) -> *mut PyObject {
    unsafe { invoke::<{ ts::Py_mp_subscript }>(receiver, 0, &[argument], false) }
}

pub(super) unsafe extern "C" fn next_not_implemented(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        errors::PyErr_Format(
            (&raw mut PyExc_TypeError).cast(),
            c"'%.200s' object is not an iterator".as_ptr(),
            (*crate::bridge::semantic_type(receiver)).tp_name,
        );
    }
    ptr::null_mut()
}

unsafe fn reflected_overrides<const SLOT: c_int>(
    left: *mut PyObject,
    right: *mut PyObject,
) -> Result<bool, ()> {
    unsafe {
        let key = name::<SLOT>(1);
        if key.as_ptr().is_null() {
            return Err(());
        }
        let left_type = crate::bridge::semantic_type(left);
        let right_type = crate::bridge::semantic_type(right);
        let right =
            OwnedPyObject::from_owned(object::PyObject_GetAttr(right_type.cast(), key.as_ptr()));
        if right.as_ptr().is_null() {
            if errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()) == 0 {
                return Err(());
            }
            errors::PyErr_Clear();
            return Ok(false);
        }
        let left =
            OwnedPyObject::from_owned(object::PyObject_GetAttr(left_type.cast(), key.as_ptr()));
        if left.as_ptr().is_null() {
            if errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()) == 0 {
                return Err(());
            }
            errors::PyErr_Clear();
            return Ok(true);
        }
        let comparison = PyObject_RichCompareBool(left.as_ptr(), right.as_ptr(), 3);
        if comparison < 0 {
            Err(())
        } else {
            Ok(comparison != 0)
        }
    }
}

unsafe fn binary_dispatch<const SLOT: c_int>(
    left: *mut PyObject,
    right: *mut PyObject,
    modulo: Option<*mut PyObject>,
) -> *mut PyObject {
    unsafe {
        let left_type = crate::bridge::semantic_type(left);
        let right_type = crate::bridge::semantic_type(right);
        let slot = stable_slot_wrapper(SLOT).unwrap();
        let target = dispatcher(slot);
        let left_dispatches = slot_wrapper_ptr(left_type, slot) == target;
        let mut right_dispatches =
            left_type != right_type && slot_wrapper_ptr(right_type, slot) == target;
        let call = |receiver, other, reverse| {
            if let Some(modulo) = modulo {
                invoke::<SLOT>(receiver, reverse, &[other, modulo], true)
            } else {
                invoke::<SLOT>(receiver, reverse, &[other], true)
            }
        };
        if left_dispatches {
            if right_dispatches && PyType_IsSubtype(right_type, left_type) != 0 {
                match reflected_overrides::<SLOT>(left, right) {
                    Err(()) => return ptr::null_mut(),
                    Ok(true) => {
                        let result = call(right, left, 1);
                        if result != &raw mut Py_NotImplementedSentinel {
                            return result;
                        }
                        crate::api::refcount::Py_DECREF(result);
                        right_dispatches = false;
                    }
                    Ok(false) => {}
                }
            }
            let result = call(left, right, 0);
            if result != &raw mut Py_NotImplementedSentinel || left_type == right_type {
                return result;
            }
            crate::api::refcount::Py_DECREF(result);
        }
        if right_dispatches {
            call(right, left, 1)
        } else {
            object::Py_NewRef(&raw mut Py_NotImplementedSentinel)
        }
    }
}
unsafe extern "C" fn binary<const SLOT: c_int>(
    left: *mut PyObject,
    right: *mut PyObject,
) -> *mut PyObject {
    unsafe { binary_dispatch::<SLOT>(left, right, None) }
}
unsafe extern "C" fn power(
    left: *mut PyObject,
    right: *mut PyObject,
    modulo: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if modulo.is_null() || modulo == &raw mut Py_None {
            binary_dispatch::<{ ts::Py_nb_power }>(left, right, None)
        } else {
            invoke::<{ ts::Py_nb_power }>(left, 0, &[right, modulo], true)
        }
    }
}
unsafe extern "C" fn inplace_power(
    left: *mut PyObject,
    right: *mut PyObject,
    modulo: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if modulo.is_null() || modulo == &raw mut Py_None {
            invoke::<{ ts::Py_nb_inplace_power }>(left, 0, &[right], true)
        } else {
            invoke::<{ ts::Py_nb_inplace_power }>(left, 0, &[right, modulo], true)
        }
    }
}

unsafe extern "C" fn length<const SLOT: c_int>(receiver: *mut PyObject) -> Py_ssize_t {
    unsafe {
        let result = OwnedPyObject::from_owned(invoke::<SLOT>(receiver, 0, &[], false));
        if result.as_ptr().is_null() {
            return -1;
        }
        let length = abstract_number::PyNumber_AsSsize_t(
            result.as_ptr(),
            (&raw mut PyExc_OverflowError).cast(),
        );
        if length < 0 && !descriptors::pending() {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"__len__() should return >= 0".as_ptr(),
            );
            return -1;
        }
        length
    }
}
unsafe extern "C" fn truth(receiver: *mut PyObject) -> c_int {
    unsafe {
        let method = match special::<{ ts::Py_nb_bool }>(receiver, 0) {
            Err(()) => return -1,
            Ok(Some(method)) => method,
            Ok(None) => {
                return match special::<{ ts::Py_sq_length }>(receiver, 0) {
                    Err(()) => -1,
                    Ok(None) => 1,
                    Ok(Some(method)) => {
                        let result =
                            OwnedPyObject::from_owned(object::PyObject_CallNoArgs(method.as_ptr()));
                        if result.as_ptr().is_null() {
                            return -1;
                        }
                        let length = abstract_number::PyNumber_AsSsize_t(
                            result.as_ptr(),
                            (&raw mut PyExc_OverflowError).cast(),
                        );
                        if length < 0 {
                            if !descriptors::pending() {
                                errors::PyErr_SetString(
                                    (&raw mut PyExc_ValueError).cast(),
                                    c"__len__() should return >= 0".as_ptr(),
                                );
                            }
                            -1
                        } else {
                            (length != 0) as c_int
                        }
                    }
                };
            }
        };
        let result = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(method.as_ptr()));
        if result.as_ptr().is_null() {
            return -1;
        }
        if crate::bridge::semantic_type(result.as_ptr()) != &raw mut PyBool_Type {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"__bool__ should return bool".as_ptr(),
            );
            return -1;
        }
        object::PyObject_IsTrue(result.as_ptr())
    }
}
unsafe extern "C" fn hash(receiver: *mut PyObject) -> isize {
    unsafe {
        let method = match special::<{ ts::Py_tp_hash }>(receiver, 0) {
            Ok(Some(method)) if method.as_ptr() != &raw mut Py_None => method,
            _ => return PyObject_HashNotImplemented(receiver),
        };
        let result = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(method.as_ptr()));
        if result.as_ptr().is_null() {
            return -1;
        }
        if numbers::PyLong_Check(result.as_ptr()) == 0 {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"__hash__ method should return an integer".as_ptr(),
            );
            return -1;
        }
        let mut hash = numbers::PyLong_AsSsize_t(result.as_ptr());
        if hash == -1 && descriptors::pending() {
            if errors::PyErr_ExceptionMatches((&raw mut PyExc_OverflowError).cast()) == 0 {
                return -1;
            }
            errors::PyErr_Clear();
            hash = PyObject_Hash(result.as_ptr());
        }
        if hash == -1 && !descriptors::pending() {
            -2
        } else {
            hash
        }
    }
}
unsafe fn ignored_result(result: *mut PyObject) -> c_int {
    let owner = unsafe { OwnedPyObject::from_owned(result) };
    if owner.as_ptr().is_null() { -1 } else { 0 }
}
unsafe extern "C" fn setitem(
    receiver: *mut PyObject,
    key: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        ignored_result(if value.is_null() {
            invoke::<{ ts::Py_mp_ass_subscript }>(receiver, 1, &[key], false)
        } else {
            invoke::<{ ts::Py_mp_ass_subscript }>(receiver, 0, &[key, value], false)
        })
    }
}
unsafe extern "C" fn getitem_index(receiver: *mut PyObject, index: Py_ssize_t) -> *mut PyObject {
    unsafe {
        let index = OwnedPyObject::from_owned(numbers::PyLong_FromSsize_t(index));
        if index.as_ptr().is_null() {
            return ptr::null_mut();
        }
        invoke::<{ ts::Py_sq_item }>(receiver, 0, &[index.as_ptr()], false)
    }
}
unsafe extern "C" fn setitem_index(
    receiver: *mut PyObject,
    index: Py_ssize_t,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        let index = OwnedPyObject::from_owned(numbers::PyLong_FromSsize_t(index));
        if index.as_ptr().is_null() {
            return -1;
        }
        ignored_result(if value.is_null() {
            invoke::<{ ts::Py_sq_ass_item }>(receiver, 1, &[index.as_ptr()], false)
        } else {
            invoke::<{ ts::Py_sq_ass_item }>(receiver, 0, &[index.as_ptr(), value], false)
        })
    }
}
unsafe extern "C" fn contains(receiver: *mut PyObject, value: *mut PyObject) -> c_int {
    unsafe {
        let method = match special::<{ ts::Py_sq_contains }>(receiver, 0) {
            Err(()) => return -1,
            Ok(None) => {
                return crate::api::abstract_sequence::iter_search(
                    receiver,
                    value,
                    crate::api::abstract_sequence::IterSearch::Contains,
                ) as c_int;
            }
            Ok(Some(method)) if method.as_ptr() == &raw mut Py_None => {
                errors::PyErr_Format(
                    (&raw mut PyExc_TypeError).cast(),
                    c"'%.200s' object is not a container".as_ptr(),
                    (*crate::bridge::semantic_type(receiver)).tp_name,
                );
                return -1;
            }
            Ok(Some(method)) => method,
        };
        let result = OwnedPyObject::from_owned(object::PyObject_CallOneArg(method.as_ptr(), value));
        if result.as_ptr().is_null() {
            -1
        } else {
            object::PyObject_IsTrue(result.as_ptr())
        }
    }
}
unsafe extern "C" fn compare(
    receiver: *mut PyObject,
    other: *mut PyObject,
    operation: c_int,
) -> *mut PyObject {
    unsafe { invoke::<{ ts::Py_tp_richcompare }>(receiver, operation as usize, &[other], true) }
}
unsafe extern "C" fn setattribute(
    receiver: *mut PyObject,
    key: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        ignored_result(if value.is_null() {
            invoke::<{ ts::Py_tp_setattro }>(receiver, 1, &[key], false)
        } else {
            invoke::<{ ts::Py_tp_setattro }>(receiver, 0, &[key, value], false)
        })
    }
}
unsafe extern "C" fn getattribute(receiver: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    unsafe {
        let result = invoke::<{ ts::Py_tp_getattro }>(receiver, 0, &[key], false);
        if !result.is_null()
            || errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()) == 0
        {
            return result;
        }
        // A missing fallback must preserve the original AttributeError.
        let raised = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        match special::<{ ts::Py_tp_getattro }>(receiver, 1) {
            Err(()) => ptr::null_mut(),
            Ok(None) => {
                errors::PyErr_SetRaisedException(raised.into_ptr());
                ptr::null_mut()
            }
            Ok(Some(method)) => object::PyObject_CallOneArg(method.as_ptr(), key),
        }
    }
}
unsafe fn not_iterable(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        errors::PyErr_Format(
            (&raw mut PyExc_TypeError).cast(),
            c"'%.200s' object is not iterable".as_ptr(),
            (*crate::bridge::semantic_type(receiver)).tp_name,
        );
    }
    ptr::null_mut()
}
unsafe extern "C" fn iter(receiver: *mut PyObject) -> *mut PyObject {
    unsafe {
        match special::<{ ts::Py_tp_iter }>(receiver, 0) {
            Ok(Some(method)) if method.as_ptr() != &raw mut Py_None => {
                object::PyObject_CallNoArgs(method.as_ptr())
            }
            Ok(Some(_)) => not_iterable(receiver),
            Err(()) | Ok(None) => {
                // slot_tp_iter clears a failed __iter__ lookup before trying
                // the sequence protocol, including descriptor lookup errors.
                errors::PyErr_Clear();
                match special::<{ ts::Py_sq_item }>(receiver, 0) {
                    Ok(Some(_)) => object::PySeqIter_New(receiver),
                    Err(()) | Ok(None) => not_iterable(receiver),
                }
            }
        }
    }
}
unsafe extern "C" fn descriptor_get(
    receiver: *mut PyObject,
    instance: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let tp = crate::bridge::semantic_type(receiver);
        if tp.is_null() {
            return ptr::null_mut();
        }
        let key = name::<{ ts::Py_tp_descr_get }>(0);
        if key.as_ptr().is_null() {
            return ptr::null_mut();
        }
        // CPython calls the raw __get__ with self explicitly; binding this
        // lookup would incorrectly invoke descriptors stored under __get__.
        let method = OwnedPyObject::from_borrowed(_PyType_Lookup(tp, key.as_ptr()));
        if method.as_ptr().is_null() {
            if descriptors::pending() {
                return ptr::null_mut();
            }
            if slot_wrapper_ptr(tp, SlotWrapper::Direct(DirectSlot::DescrGet))
                == dispatcher(SlotWrapper::Direct(DirectSlot::DescrGet))
            {
                (*tp).tp_descr_get = None;
            }
            return object::Py_NewRef(receiver);
        }
        let mut arguments = [
            receiver,
            if instance.is_null() {
                &raw mut Py_None
            } else {
                instance
            },
            if owner.is_null() {
                &raw mut Py_None
            } else {
                owner
            },
        ];
        object::PyObject_VectorcallDict(
            method.as_ptr(),
            arguments.as_mut_ptr(),
            arguments.len(),
            ptr::null_mut(),
        )
    }
}
unsafe extern "C" fn descriptor_set(
    receiver: *mut PyObject,
    instance: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        ignored_result(if value.is_null() {
            invoke::<{ ts::Py_tp_descr_set }>(receiver, 1, &[instance], false)
        } else {
            invoke::<{ ts::Py_tp_descr_set }>(receiver, 0, &[instance, value], false)
        })
    }
}
unsafe fn call_tuple<const SLOT: c_int>(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        match special::<SLOT>(receiver, 0) {
            Err(()) => ptr::null_mut(),
            Ok(None) => {
                errors::PyErr_SetString(
                    (&raw mut PyExc_AttributeError).cast(),
                    c"special method is missing".as_ptr(),
                );
                ptr::null_mut()
            }
            Ok(Some(method)) => object::PyObject_Call(method.as_ptr(), args, kwargs),
        }
    }
}
unsafe extern "C" fn call(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe { call_tuple::<{ ts::Py_tp_call }>(receiver, args, kwargs) }
}
unsafe extern "C" fn init(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> c_int {
    unsafe {
        let result =
            OwnedPyObject::from_owned(call_tuple::<{ ts::Py_tp_init }>(receiver, args, kwargs));
        if result.as_ptr().is_null() {
            return -1;
        }
        if result.as_ptr() != &raw mut Py_None {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"__init__() should return None".as_ptr(),
            );
            return -1;
        }
        0
    }
}
unsafe extern "C" fn new(
    tp: *mut PyTypeObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let key = name::<{ ts::Py_tp_new }>(0);
        if key.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let method = OwnedPyObject::from_owned(object::PyObject_GetAttr(tp.cast(), key.as_ptr()));
        if method.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let count = sequences::PyTuple_Size(args);
        if count < 0 {
            return ptr::null_mut();
        }
        let mut arguments = Vec::with_capacity(count as usize + 1);
        arguments.push(tp.cast());
        for index in 0..count {
            arguments.push(sequences::PyTuple_GetItem(args, index));
        }
        object::PyObject_VectorcallDict(
            method.as_ptr(),
            arguments.as_mut_ptr(),
            arguments.len(),
            kwargs,
        )
    }
}
unsafe extern "C" fn finalize(receiver: *mut PyObject) {
    errors::with_preserved_error(|| unsafe {
        if let Ok(Some(method)) = special::<{ ts::Py_tp_finalize }>(receiver, 0) {
            let result = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(method.as_ptr()));
            if result.as_ptr().is_null() {
                errors::PyErr_WriteUnraisable(method.as_ptr());
            }
        }
        // Missing __del__ and failed optional lookup are silent. The saved
        // exception is restored by with_preserved_error, as slot_tp_finalize.
    });
}
