//! Descriptor protocol shared by native attribute lookup and Molt crossings.
//!
//! Managed values use their live runtime class; foreign slots belong to the
//! physical type. Absence differs from failure, and NULL operands retain their C meaning.
//! Every callback return publishes mutable operand projections before runtime
//! observation, including mutations made before a callback raises.

use crate::abi_types::{PyDescrGetFunc, PyDescrSetFunc, PyObject};
use crate::api::callback::CallbackOperands;
use crate::api::errors;
use crate::bridge::{GLOBAL_BRIDGE, RuntimeValue};
use crate::hooks::{DecodedHandleResult, DescriptorMutationStatus, DescriptorProtocol, hooks_or_stubs};
use std::os::raw::c_int;
use std::ptr;

unsafe fn protocol(descriptor: *mut PyObject) -> Result<DescriptorProtocol, ()> {
    if descriptor.is_null() {
        return Ok(DescriptorProtocol::None);
    }
    // This canonical identity query excludes address.foreign/raw_py entries:
    // acquiring a runtime wrapper never turns a native descriptor into a
    // managed view or redispatches its physical slot back through that wrapper.
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(descriptor).is_some() {
        let value = unsafe { RuntimeValue::acquire(descriptor) }.ok_or(())?;
        let protocol = unsafe { (hooks_or_stubs().descriptor_protocol)(value.bits()) };
        return if protocol == DescriptorProtocol::Error || errors::raised_error_pending() {
            unsafe { crate::bridge::ensure_result_error(c"managed descriptor protocol failed") };
            Err(())
        } else {
            Ok(protocol)
        };
    }
    let Some(kind) = (unsafe { (*descriptor).ob_type.as_ref() }) else {
        return Ok(DescriptorProtocol::None);
    };
    Ok(DescriptorProtocol::from_slots(kind.tp_descr_get.is_some(), kind.tp_descr_set.is_some()))
}

pub(crate) unsafe fn has_get(descriptor: *mut PyObject) -> Result<bool, ()> {
    unsafe { protocol(descriptor) }.map(DescriptorProtocol::has_get)
}

pub(crate) unsafe fn is_data(descriptor: *mut PyObject) -> Result<bool, ()> {
    unsafe { protocol(descriptor) }.map(DescriptorProtocol::is_data)
}

/// Invoke an exact get callback. Published descriptor slots and explicit
/// __get__ wrappers share this path without redispatching a declaring callback.
pub(crate) unsafe fn invoke_get(
    slot: PyDescrGetFunc,
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    let operands = unsafe { CallbackOperands::new([descriptor, receiver, owner]) };
    let result = unsafe { slot(descriptor, receiver, owner) };
    unsafe { operands.complete_result(result, "native descriptor get") }
}

/// Invoke an exact set/delete callback, retaining NULL deletion and every
/// partial mutation even when the callback fails. Owned operands outlive both
/// the callback and the canonical synchronization transaction.
pub(crate) unsafe fn invoke_set(
    slot: PyDescrSetFunc,
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    let operands = unsafe { CallbackOperands::new([descriptor, receiver, value]) };
    let status = unsafe { slot(descriptor, receiver, value) };
    unsafe { operands.complete_status(status, "native descriptor mutation") }
}

/// Invoke the native get slot, retaining its borrowed class-dictionary entry
/// and borrowed callback operands across replacement and synchronization.
pub(crate) unsafe fn get(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> Option<*mut PyObject> {
    if descriptor.is_null() {
        return None;
    }
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(descriptor).is_some() {
        match unsafe { has_get(descriptor) } {
            Ok(true) => {},
            Ok(false) => return None,
            Err(()) => return Some(ptr::null_mut()),
        }
        let operands = unsafe { CallbackOperands::new([descriptor, receiver, owner]) };
        let Some(descriptor) = (unsafe { RuntimeValue::acquire(descriptor) }) else {
            return Some(ptr::null_mut());
        };
        let receiver = match unsafe { optional_value(receiver) } {
            Ok(value) => value,
            Err(()) => return Some(ptr::null_mut()),
        };
        let owner = match unsafe { optional_value(owner) } {
            Ok(value) => value,
            Err(()) => return Some(ptr::null_mut()),
        };
        let receiver_bits = receiver.as_ref().map(RuntimeValue::bits);
        let owner_bits = owner.as_ref().map(RuntimeValue::bits);
        let result = unsafe {
            (hooks_or_stubs().descriptor_get)(
                descriptor.bits(),
                receiver_bits.as_ref().map_or(ptr::null(), |bits| bits),
                owner_bits.as_ref().map_or(ptr::null(), |bits| bits),
            )
        };
        if matches!(result.decode(), DecodedHandleResult::Missing) {
            return None;
        }
        let result = unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
        return Some(unsafe { operands.complete_result(result, "managed descriptor get") });
    }
    let kind = unsafe { (*descriptor).ob_type.as_ref() }?;
    let slot = kind.tp_descr_get?;
    Some(unsafe { invoke_get(slot, descriptor, receiver, owner) })
}

unsafe fn optional_value(value: *mut PyObject) -> Result<Option<RuntimeValue>, ()> {
    if value.is_null() {
        Ok(None)
    } else {
        unsafe { RuntimeValue::acquire(value) }.map(Some).ok_or(())
    }
}

/// A NULL value means delete; Python None is an ordinary non-NULL value.
pub(crate) unsafe fn set(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    value: *mut PyObject,
) -> Option<c_int> {
    if descriptor.is_null() {
        return None;
    }
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(descriptor).is_some() {
        match unsafe { is_data(descriptor) } {
            Ok(true) => {},
            Ok(false) => return None,
            Err(()) => return Some(-1),
        }
        let operands = unsafe { CallbackOperands::new([descriptor, receiver, value]) };
        let Some(descriptor) = (unsafe { RuntimeValue::acquire(descriptor) }) else {
            return Some(-1);
        };
        let Some(receiver) = (unsafe { RuntimeValue::acquire(receiver) }) else {
            return Some(-1);
        };
        let value = match unsafe { optional_value(value) } {
            Ok(value) => value,
            Err(()) => return Some(-1),
        };
        let value_bits = value.as_ref().map(RuntimeValue::bits);
        let status = unsafe {
            (hooks_or_stubs().descriptor_set)(
                descriptor.bits(), receiver.bits(),
                value_bits.as_ref().map_or(ptr::null(), |bits| bits),
            )
        };
        let status = match status {
            DescriptorMutationStatus::Missing => return None,
            DescriptorMutationStatus::Applied => 0,
            DescriptorMutationStatus::Error => -1,
        };
        return Some(unsafe { operands.complete_status(status, "managed descriptor mutation") });
    }
    let kind = unsafe { (*descriptor).ob_type.as_ref() }?;
    let slot = kind.tp_descr_set?;
    Some(unsafe { invoke_set(slot, descriptor, receiver, value) })
}

/// Optional C attribute lookup suppresses only AttributeError and subclasses.
/// Failure from the descriptor's body remains observable for every other type.
pub(crate) unsafe fn suppress_attribute_error(suppress: c_int, result: *mut PyObject) {
    if suppress != 0
        && result.is_null()
        && unsafe {
            errors::PyErr_ExceptionMatches((&raw mut crate::abi_types::PyExc_AttributeError).cast())
        } != 0
    {
        unsafe { errors::PyErr_Clear() };
    }
}
