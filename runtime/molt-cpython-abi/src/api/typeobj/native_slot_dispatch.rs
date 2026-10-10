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
) -> Result<Option<OwnedPyObject>, crate::ErrorIndicatorSet> {
    let key = unsafe { name::<SLOT>(variant) };
    if key.as_ptr().is_null() {
        return Err(crate::ErrorIndicatorSet);
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
        Err(crate::ErrorIndicatorSet) => ptr::null_mut(),
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
            Err(crate::ErrorIndicatorSet) | Ok(None) => {
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
            Err(crate::ErrorIndicatorSet) | Ok(None) => {
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
) -> Result<bool, crate::ErrorIndicatorSet> {
    unsafe {
        let key = name::<SLOT>(1);
        if key.as_ptr().is_null() {
            return Err(crate::ErrorIndicatorSet);
        }
        let left_type = crate::bridge::semantic_type(left);
        let right_type = crate::bridge::semantic_type(right);
        let right =
            OwnedPyObject::from_owned(object::PyObject_GetAttr(right_type.cast(), key.as_ptr()));
        if right.as_ptr().is_null() {
            if errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()) == 0 {
                return Err(crate::ErrorIndicatorSet);
            }
            errors::PyErr_Clear();
            return Ok(false);
        }
        let left =
            OwnedPyObject::from_owned(object::PyObject_GetAttr(left_type.cast(), key.as_ptr()));
        if left.as_ptr().is_null() {
            if errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()) == 0 {
                return Err(crate::ErrorIndicatorSet);
            }
            errors::PyErr_Clear();
            return Ok(true);
        }
        let comparison = PyObject_RichCompareBool(left.as_ptr(), right.as_ptr(), 3);
        if comparison < 0 {
            Err(crate::ErrorIndicatorSet)
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
                    Err(crate::ErrorIndicatorSet) => return ptr::null_mut(),
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
            return binary_dispatch::<{ ts::Py_nb_power }>(left, right, None);
        }
        let minor = (crate::hooks::hooks_or_stubs().target_python_minor)();
        if minor < 0 {
            if errors::PyErr_Occurred().is_null() {
                errors::PyErr_SetString(
                    (&raw mut PyExc_SystemError).cast(),
                    c"semantic Python target unavailable".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        if minor >= 14 {
            binary_dispatch::<{ ts::Py_nb_power }>(left, right, Some(modulo))
        } else {
            // Older slot_nb_power is left-only even when the raw ternary
            // engine reached this wrapper through the exponent/modulus type.
            let ty = crate::bridge::semantic_type(left);
            let slot = stable_slot_wrapper(ts::Py_nb_power).unwrap();
            if slot_wrapper_ptr(ty, slot) != dispatcher(slot) {
                return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
            }
            invoke::<{ ts::Py_nb_power }>(left, 0, &[right, modulo], true)
        }
    }
}
unsafe extern "C" fn inplace_power(
    left: *mut PyObject,
    right: *mut PyObject,
    _modulo: *mut PyObject,
) -> *mut PyObject {
    // Python-defined __ipow__ is binary in every declared semantic target.
    // The raw C ternary slot and its explicit descriptor wrapper stay ternary.
    unsafe { invoke::<{ ts::Py_nb_inplace_power }>(left, 0, &[right], true) }
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
            Err(crate::ErrorIndicatorSet) => return -1,
            Ok(Some(method)) => method,
            Ok(None) => {
                return match special::<{ ts::Py_sq_length }>(receiver, 0) {
                    Err(crate::ErrorIndicatorSet) => -1,
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
            Err(crate::ErrorIndicatorSet) => return -1,
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
            Err(crate::ErrorIndicatorSet) => ptr::null_mut(),
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
            Err(crate::ErrorIndicatorSet) | Ok(None) => {
                // slot_tp_iter clears a failed __iter__ lookup before trying
                // the sequence protocol, including descriptor lookup errors.
                errors::PyErr_Clear();
                match special::<{ ts::Py_sq_item }>(receiver, 0) {
                    Ok(Some(_)) => object::PySeqIter_New(receiver),
                    Err(crate::ErrorIndicatorSet) | Ok(None) => not_iterable(receiver),
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
            Err(crate::ErrorIndicatorSet) => ptr::null_mut(),
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

/// Invoke the physical descriptor declared by this immutable builtin, without
/// looking through the operand's override. Names/ABIs remain SLOT_WRAPPER_DEFS.
unsafe fn declaring_numeric_call<const SLOT: c_int>(
    declaring: *mut PyTypeObject,
    args: &[*mut PyObject],
) -> *mut PyObject {
    unsafe {
        let key = name::<SLOT>(0);
        if key.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let descriptor = type_namespace_lookup_with_bridge(
            &crate::bridge::GLOBAL_BRIDGE,
            declaring,
            key.as_ptr(),
        );
        if descriptor.is_null() {
            if errors::PyErr_Occurred().is_null() {
                errors::PyErr_SetString(
                    (&raw mut PyExc_SystemError).cast(),
                    c"declared builtin numeric slot unavailable".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        let descriptor = OwnedPyObject::from_borrowed(descriptor);
        object::PyObject_Vectorcall(
            descriptor.as_ptr(),
            args.as_ptr().cast_mut(),
            args.len(),
            ptr::null_mut(),
        )
    }
}

// These slots invoke the namespace of their declaring builtin, not the
// physical carrier fallback selected by bridge::tag_to_type. In particular,
// int's slot still declares int when its receiver is a bool.
fn numeric_declaring_type<const TAG: u8>() -> *mut PyTypeObject {
    match TAG {
        value if value == MoltTypeTag::Int as u8 => &raw mut PyLong_Type,
        value if value == MoltTypeTag::Bool as u8 => &raw mut PyBool_Type,
        value if value == MoltTypeTag::Float as u8 => &raw mut PyFloat_Type,
        value if value == MoltTypeTag::Complex as u8 => &raw mut PyComplex_Type,
        _ => unreachable!("builtin numeric table tag"),
    }
}

unsafe fn numeric_operand_admitted<const TAG: u8>(value: *mut PyObject) -> bool {
    unsafe {
        numbers::PyLong_Check(value) != 0
            || (TAG == MoltTypeTag::Float as u8 || TAG == MoltTypeTag::Complex as u8)
                && numbers::PyFloat_Check(value) != 0
            || TAG == MoltTypeTag::Complex as u8 && numbers::PyComplex_Check(value) != 0
    }
}

/// Existing managed values keep their original C identity. A genuine foreign
/// numeric prefix uses the same callback-free storage projection as comparison.
unsafe fn native_numeric_argument(value: *mut PyObject) -> OwnedPyObject {
    unsafe {
        match crate::bridge::observe_pyobject(value) {
            Some(crate::bridge::ResolvedPyObject::ManagedMolt(_)) => {
                return OwnedPyObject::from_borrowed(value);
            }
            Some(crate::bridge::ResolvedPyObject::Foreign) => {}
            None => return OwnedPyObject::from_owned(ptr::null_mut()),
        }
        match numbers::numeric_native_value(value) {
            Some(value) => OwnedPyObject::from_owned(
                crate::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(value.into_owned_bits()),
            ),
            None => OwnedPyObject::from_owned(ptr::null_mut()),
        }
    }
}

/// A raw float/complex slot can be selected through a right operand or modulus.
/// Admit its left carrier through the declared conversion, never __float__ or
/// __complex__ on the operand's dynamic class.
unsafe fn numeric_declaring_receiver<const TAG: u8>(value: *mut PyObject) -> OwnedPyObject {
    unsafe {
        let value = native_numeric_argument(value);
        if value.as_ptr().is_null() {
            return value;
        }
        if TAG == MoltTypeTag::Float as u8 && numbers::PyFloat_Check(value.as_ptr()) == 0 {
            return OwnedPyObject::from_owned(declaring_numeric_call::<{ ts::Py_nb_float }>(
                &raw mut PyLong_Type,
                &[value.as_ptr()],
            ));
        }
        if TAG == MoltTypeTag::Complex as u8 && numbers::PyComplex_Check(value.as_ptr()) == 0 {
            let declaring = if numbers::PyLong_Check(value.as_ptr()) != 0 {
                &raw mut PyLong_Type
            } else {
                &raw mut PyFloat_Type
            };
            let real = OwnedPyObject::from_owned(declaring_numeric_call::<{ ts::Py_nb_float }>(
                declaring,
                &[value.as_ptr()],
            ));
            if real.as_ptr().is_null() {
                return real;
            }
            let real_value = numbers::PyFloat_AsDouble(real.as_ptr());
            if !errors::PyErr_Occurred().is_null() {
                return OwnedPyObject::from_owned(ptr::null_mut());
            }
            return OwnedPyObject::from_owned(numbers::PyComplex_FromDoubles(real_value, 0.0));
        }
        value
    }
}

unsafe extern "C" fn native_number_binary<const TAG: u8, const SLOT: c_int>(
    a: *mut PyObject,
    b: *mut PyObject,
) -> *mut PyObject {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe {
        if !numeric_operand_admitted::<TAG>(a) {
            return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
        }
        if TAG == MoltTypeTag::Bool as u8
            && (a != (&raw mut Py_True).cast() && a != (&raw mut Py_False).cast()
                || b != (&raw mut Py_True).cast() && b != (&raw mut Py_False).cast())
        {
            return native_number_binary::<{ MoltTypeTag::Int as u8 }, SLOT>(a, b);
        }
        let left = numeric_declaring_receiver::<TAG>(a);
        if left.as_ptr().is_null() {
            return ptr::null_mut();
        }
        if !numeric_operand_admitted::<TAG>(b) {
            return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
        }
        let right = native_numeric_argument(b);
        if right.as_ptr().is_null() {
            return ptr::null_mut();
        }
        declaring_numeric_call::<SLOT>(
            numeric_declaring_type::<TAG>(),
            &[left.as_ptr(), right.as_ptr()],
        )
    }
}

unsafe extern "C" fn native_number_power<const TAG: u8>(
    a: *mut PyObject,
    b: *mut PyObject,
    modulus: *mut PyObject,
) -> *mut PyObject {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe {
        if TAG == MoltTypeTag::Float as u8 && !modulus.is_null() && modulus != &raw mut Py_None {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"pow() 3rd argument not allowed unless all arguments are integers".as_ptr(),
            );
            return ptr::null_mut();
        }
        if !numeric_operand_admitted::<TAG>(a)
            || TAG == MoltTypeTag::Int as u8
                && !modulus.is_null()
                && modulus != &raw mut Py_None
                && numbers::PyLong_Check(modulus) == 0
        {
            return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
        }
        let left = numeric_declaring_receiver::<TAG>(a);
        if left.as_ptr().is_null() {
            return ptr::null_mut();
        }
        if !numeric_operand_admitted::<TAG>(b) {
            return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
        }
        let right = native_numeric_argument(b);
        if right.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let modulus = if modulus.is_null() || modulus == &raw mut Py_None {
            OwnedPyObject::from_borrowed(&raw mut Py_None)
        } else if numbers::PyLong_Check(modulus) != 0 {
            native_numeric_argument(modulus)
        } else {
            OwnedPyObject::from_borrowed(modulus)
        };
        if modulus.as_ptr().is_null() {
            return ptr::null_mut();
        }
        declaring_numeric_call::<{ ts::Py_nb_power }>(
            numeric_declaring_type::<TAG>(),
            &[left.as_ptr(), right.as_ptr(), modulus.as_ptr()],
        )
    }
}

unsafe extern "C" fn native_number_unary<const TAG: u8, const SLOT: c_int>(
    a: *mut PyObject,
) -> *mut PyObject {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe {
        let argument = numeric_declaring_receiver::<TAG>(a);
        if argument.as_ptr().is_null() {
            return ptr::null_mut();
        }
        declaring_numeric_call::<SLOT>(numeric_declaring_type::<TAG>(), &[argument.as_ptr()])
    }
}
unsafe extern "C" fn native_number_bool<const TAG: u8>(a: *mut PyObject) -> c_int {
    unsafe {
        let result = OwnedPyObject::from_owned(native_number_unary::<TAG, { ts::Py_nb_bool }>(a));
        if result.as_ptr().is_null() {
            -1
        } else {
            object::PyObject_IsTrue(result.as_ptr())
        }
    }
}

/// Container number slots reuse their declaring physical runtime descriptors.
/// Set and frozenset share the same raw binary slot identity, as CPython does.
unsafe extern "C" fn native_container_binary<const DICT: bool, const SLOT: c_int>(
    a: *mut PyObject,
    b: *mut PyObject,
) -> *mut PyObject {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe {
        let inplace = matches!(
            SLOT,
            ts::Py_nb_inplace_or
                | ts::Py_nb_inplace_and
                | ts::Py_nb_inplace_subtract
                | ts::Py_nb_inplace_xor
        );
        let admitted = |value| {
            if DICT {
                crate::api::mapping::PyDict_Check(value) != 0
            } else {
                sequences::PySet_Check(value) != 0 || sequences::PyFrozenSet_Check(value) != 0
            }
        };
        if !admitted(a)
            || !(DICT && inplace) && !admitted(b)
            || !DICT && inplace && sequences::PySet_Check(a) == 0
        {
            return object::Py_NewRef(&raw mut Py_NotImplementedSentinel);
        }
        // Constructors publish one runtime container backing. A foreign prefix
        // with a builtin name alone does not acquire that storage authority.
        for value in [Some(a), (!(DICT && inplace)).then_some(b)]
            .into_iter()
            .flatten()
        {
            match crate::bridge::observe_pyobject(value) {
                Some(crate::bridge::ResolvedPyObject::ManagedMolt(_)) => {}
                Some(crate::bridge::ResolvedPyObject::Foreign) => {
                    errors::PyErr_SetString(
                        (&raw mut PyExc_SystemError).cast(),
                        c"builtin container numeric slot requires runtime container storage"
                            .as_ptr(),
                    );
                    return ptr::null_mut();
                }
                None => return ptr::null_mut(),
            }
        }
        let declaring = if DICT {
            &raw mut PyDict_Type
        } else if sequences::PyFrozenSet_Check(a) != 0 {
            &raw mut PyFrozenSet_Type
        } else {
            &raw mut PySet_Type
        };
        declaring_numeric_call::<SLOT>(declaring, &[a, b])
    }
}

/// The actual static ABI projection. Immutable numeric types have no in-place
/// or matrix-multiply slots; bool inherits every int pointer except its four
/// declared overrides. Python method names remain in the canonical slot rows.
pub(super) fn builtin_number_table<const TAG: u8>() -> PyNumberMethods {
    let mut table: PyNumberMethods = unsafe { std::mem::zeroed() };
    macro_rules! binary {
        ($field:ident, $slot:ident) => {
            table.$field = native_number_binary::<TAG, { ts::$slot }> as *const () as *mut c_void;
        };
    }
    macro_rules! unary {
        ($field:ident, $slot:ident) => {
            table.$field = native_number_unary::<TAG, { ts::$slot }> as *const () as *mut c_void;
        };
    }
    if TAG == MoltTypeTag::Dict as u8 {
        table.nb_or = native_container_binary::<true, { ts::Py_nb_or }> as *const () as *mut c_void;
        table.nb_inplace_or =
            native_container_binary::<true, { ts::Py_nb_inplace_or }> as *const () as *mut c_void;
        return table;
    }
    if TAG == MoltTypeTag::Set as u8 || TAG == MoltTypeTag::FrozenSet as u8 {
        table.nb_subtract =
            native_container_binary::<false, { ts::Py_nb_subtract }> as *const () as *mut c_void;
        table.nb_and =
            native_container_binary::<false, { ts::Py_nb_and }> as *const () as *mut c_void;
        table.nb_or =
            native_container_binary::<false, { ts::Py_nb_or }> as *const () as *mut c_void;
        table.nb_xor =
            native_container_binary::<false, { ts::Py_nb_xor }> as *const () as *mut c_void;
        if TAG == MoltTypeTag::Set as u8 {
            table.nb_inplace_subtract =
                native_container_binary::<false, { ts::Py_nb_inplace_subtract }> as *const ()
                    as *mut c_void;
            table.nb_inplace_and = native_container_binary::<false, { ts::Py_nb_inplace_and }>
                as *const () as *mut c_void;
            table.nb_inplace_or = native_container_binary::<false, { ts::Py_nb_inplace_or }>
                as *const () as *mut c_void;
            table.nb_inplace_xor = native_container_binary::<false, { ts::Py_nb_inplace_xor }>
                as *const () as *mut c_void;
        }
        return table;
    }
    if TAG == MoltTypeTag::Bool as u8 {
        table = builtin_number_table::<{ MoltTypeTag::Int as u8 }>();
        binary!(nb_and, Py_nb_and);
        binary!(nb_or, Py_nb_or);
        binary!(nb_xor, Py_nb_xor);
        unary!(nb_invert, Py_nb_invert);
        return table;
    }
    binary!(nb_add, Py_nb_add);
    binary!(nb_subtract, Py_nb_subtract);
    binary!(nb_multiply, Py_nb_multiply);
    binary!(nb_true_divide, Py_nb_true_divide);
    table.nb_power = native_number_power::<TAG> as *const () as *mut c_void;
    unary!(nb_negative, Py_nb_negative);
    unary!(nb_positive, Py_nb_positive);
    unary!(nb_absolute, Py_nb_absolute);
    table.nb_bool = native_number_bool::<TAG> as *const () as *mut c_void;
    if TAG != MoltTypeTag::Complex as u8 {
        binary!(nb_remainder, Py_nb_remainder);
        binary!(nb_divmod, Py_nb_divmod);
        binary!(nb_floor_divide, Py_nb_floor_divide);
        unary!(nb_int, Py_nb_int);
        unary!(nb_float, Py_nb_float);
    }
    if TAG == MoltTypeTag::Int as u8 {
        unary!(nb_invert, Py_nb_invert);
        unary!(nb_index, Py_nb_index);
        binary!(nb_lshift, Py_nb_lshift);
        binary!(nb_rshift, Py_nb_rshift);
        binary!(nb_and, Py_nb_and);
        binary!(nb_or, Py_nb_or);
        binary!(nb_xor, Py_nb_xor);
    }
    table
}
