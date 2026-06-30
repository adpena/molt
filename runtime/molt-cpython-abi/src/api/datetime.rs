//! Datetime C-API constructors and packed object layouts.

use crate::abi_types::{
    Py_None, Py_ssize_t, PyDateTime_Date, PyDateTime_DateTime, PyDateTime_DateTimeType,
    PyDateTime_DateType, PyDateTime_Delta, PyDateTime_DeltaType, PyDateTime_Time,
    PyDateTime_TimeType, PyObject, PyTypeObject,
};
use std::os::raw::{c_char, c_int};
use std::ptr;

fn valid_date(year: c_int, month: c_int, day: c_int) -> bool {
    (1..=9999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

fn valid_time(hour: c_int, minute: c_int, second: c_int, usecond: c_int) -> bool {
    (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=59).contains(&second)
        && (0..=999_999).contains(&usecond)
}

unsafe fn selected_type(
    requested: *mut PyTypeObject,
    fallback: *mut PyTypeObject,
) -> *mut PyTypeObject {
    if requested.is_null() {
        fallback
    } else {
        requested
    }
}

unsafe fn set_value_error(message: &'static std::ffi::CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_ValueError,
            message.as_ptr(),
        );
    }
}

unsafe fn set_not_implemented(message: &'static std::ffi::CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_NotImplementedError,
            message.as_ptr(),
        );
    }
}

fn write_date_data(data: &mut [u8], offset: usize, year: c_int, month: c_int, day: c_int) {
    data[offset] = ((year >> 8) & 0xff) as u8;
    data[offset + 1] = (year & 0xff) as u8;
    data[offset + 2] = month as u8;
    data[offset + 3] = day as u8;
}

fn write_time_data(
    data: &mut [u8],
    offset: usize,
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
) {
    data[offset] = hour as u8;
    data[offset + 1] = minute as u8;
    data[offset + 2] = second as u8;
    data[offset + 3] = ((usecond >> 16) & 0xff) as u8;
    data[offset + 4] = ((usecond >> 8) & 0xff) as u8;
    data[offset + 5] = (usecond & 0xff) as u8;
}

unsafe fn alloc_datetime_object<T>(typeobj: *mut PyTypeObject) -> *mut T {
    unsafe { crate::api::memory::molt_object_alloc(typeobj, 0).cast::<T>() }
}

unsafe fn own_tzinfo(object_tzinfo: *mut *mut PyObject, tzinfo: *mut PyObject) -> c_char {
    if tzinfo.is_null() || std::ptr::eq(tzinfo, &raw mut Py_None) {
        0
    } else {
        unsafe { crate::api::refcount::Py_INCREF(tzinfo) };
        unsafe {
            *object_tzinfo = tzinfo;
        }
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_date_from_date(
    year: c_int,
    month: c_int,
    day: c_int,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    if !valid_date(year, month, day) {
        unsafe { set_value_error(c"invalid date") };
        return ptr::null_mut();
    }
    let typeobj = unsafe { selected_type(typeobj, &raw mut PyDateTime_DateType) };
    let obj = unsafe { alloc_datetime_object::<PyDateTime_Date>(typeobj) };
    if obj.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*obj).hashcode = -1;
        (*obj).hastzinfo = 0;
        write_date_data(&mut (*obj).data, 0, year, month, day);
    }
    obj.cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_datetime_from_date_and_time(
    year: c_int,
    month: c_int,
    day: c_int,
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
    tzinfo: *mut PyObject,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    unsafe {
        molt_cpython_abi_datetime_from_date_and_time_and_fold(
            year, month, day, hour, minute, second, usecond, tzinfo, 0, typeobj,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_datetime_from_date_and_time_and_fold(
    year: c_int,
    month: c_int,
    day: c_int,
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
    tzinfo: *mut PyObject,
    fold: c_int,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    if !valid_date(year, month, day) || !valid_time(hour, minute, second, usecond) {
        unsafe { set_value_error(c"invalid datetime") };
        return ptr::null_mut();
    }
    let typeobj = unsafe { selected_type(typeobj, &raw mut PyDateTime_DateTimeType) };
    let obj = unsafe { alloc_datetime_object::<PyDateTime_DateTime>(typeobj) };
    if obj.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*obj).hashcode = -1;
        write_date_data(&mut (*obj).data, 0, year, month, day);
        write_time_data(&mut (*obj).data, 4, hour, minute, second, usecond);
        (*obj).fold = (fold != 0) as u8;
        (*obj).hastzinfo = own_tzinfo(&raw mut (*obj).tzinfo, tzinfo);
    }
    obj.cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_time_from_time(
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
    tzinfo: *mut PyObject,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    unsafe {
        molt_cpython_abi_time_from_time_and_fold(hour, minute, second, usecond, tzinfo, 0, typeobj)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_time_from_time_and_fold(
    hour: c_int,
    minute: c_int,
    second: c_int,
    usecond: c_int,
    tzinfo: *mut PyObject,
    fold: c_int,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    if !valid_time(hour, minute, second, usecond) {
        unsafe { set_value_error(c"invalid time") };
        return ptr::null_mut();
    }
    let typeobj = unsafe { selected_type(typeobj, &raw mut PyDateTime_TimeType) };
    let obj = unsafe { alloc_datetime_object::<PyDateTime_Time>(typeobj) };
    if obj.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*obj).hashcode = -1;
        write_time_data(&mut (*obj).data, 0, hour, minute, second, usecond);
        (*obj).fold = (fold != 0) as u8;
        (*obj).hastzinfo = own_tzinfo(&raw mut (*obj).tzinfo, tzinfo);
    }
    obj.cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_delta_from_delta(
    days: c_int,
    seconds: c_int,
    useconds: c_int,
    _normalize: c_int,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    let typeobj = unsafe { selected_type(typeobj, &raw mut PyDateTime_DeltaType) };
    let obj = unsafe { alloc_datetime_object::<PyDateTime_Delta>(typeobj) };
    if obj.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*obj).hashcode = -1;
        (*obj).days = days;
        (*obj).seconds = seconds;
        (*obj).microseconds = useconds;
    }
    obj.cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_timezone_from_timezone(
    _offset: *mut PyObject,
    _name: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_not_implemented(c"timezone construction requires runtime datetime hooks") };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_datetime_from_timestamp(
    _typeobj: *mut PyObject,
    _args: *mut PyObject,
    _kw: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_not_implemented(c"datetime timestamp construction requires runtime hooks") };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_date_from_timestamp(
    _typeobj: *mut PyObject,
    _args: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_not_implemented(c"date timestamp construction requires runtime hooks") };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_datetime_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let typeobj = (*op).ob_type;
        if std::ptr::eq(typeobj, &raw mut PyDateTime_DateTimeType) {
            let dt = op.cast::<PyDateTime_DateTime>();
            if (*dt).hastzinfo != 0 {
                crate::api::refcount::Py_XDECREF((*dt).tzinfo);
            }
        } else if std::ptr::eq(typeobj, &raw mut PyDateTime_TimeType) {
            let time = op.cast::<PyDateTime_Time>();
            if (*time).hastzinfo != 0 {
                crate::api::refcount::Py_XDECREF((*time).tzinfo);
            }
        }
        crate::api::memory::PyMem_Free(op.cast());
    }
}

#[allow(dead_code)]
const _: Py_ssize_t = std::mem::size_of::<PyDateTime_DateTime>() as Py_ssize_t;
