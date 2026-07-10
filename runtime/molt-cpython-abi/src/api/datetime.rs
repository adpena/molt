//! Datetime C-API constructors and packed object layouts.

use crate::abi_types::{
    Py_None, Py_ssize_t, PyDateTime_Date, PyDateTime_DateTime, PyDateTime_DateTimeType,
    PyDateTime_DateType, PyDateTime_Delta, PyDateTime_DeltaType, PyDateTime_Time,
    PyDateTime_TimeType, PyObject, PyTypeObject,
};
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Leap-aware days-in-month, mirroring `_datetimemodule.c` `days_in_month`.
fn is_leap_year(year: c_int) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: c_int, month: c_int) -> c_int {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// CPython `check_date_args`: year 1..=9999, month 1..=12, and day validated
/// against the LEAP-AWARE month length — Feb 30 / Apr 31 / Feb 29 in non-leap
/// years are ValueError, never a constructed impossible date.
fn valid_date(year: c_int, month: c_int, day: c_int) -> bool {
    (1..=9999).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
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

unsafe fn set_tzinfo_type_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"tzinfo argument must be None or of a tzinfo subclass".as_ptr(),
        );
    }
}

unsafe fn set_overflow_error(message: &'static std::ffi::CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_OverflowError,
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

/// CPython `check_tzinfo_subclass`: tzinfo must be None or a tzinfo instance
/// (`PyTZInfo_Check` — a subtype walk against `PyDateTime_TZInfoType`).
unsafe fn tzinfo_acceptable(tzinfo: *mut PyObject) -> bool {
    if tzinfo.is_null() || std::ptr::eq(tzinfo, &raw mut Py_None) {
        return true;
    }
    unsafe {
        crate::api::typeobj::PyObject_TypeCheck(
            tzinfo,
            &raw mut crate::abi_types::PyDateTime_TZInfoType,
        ) == 1
    }
}

/// CPython `check_time_args` fold clause: fold must be exactly 0 or 1.
unsafe fn valid_fold(fold: c_int) -> bool {
    if fold == 0 || fold == 1 {
        return true;
    }
    unsafe { set_value_error(c"fold must be either 0 or 1") };
    false
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
    // CPython check_tzinfo_subclass + check_time_args: a non-tzinfo object is a
    // TypeError and fold outside {0, 1} is a ValueError — both BEFORE allocation.
    if !unsafe { tzinfo_acceptable(tzinfo) } {
        unsafe { set_tzinfo_type_error() };
        return ptr::null_mut();
    }
    if !unsafe { valid_fold(fold) } {
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
        (*obj).fold = fold as u8;
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
    if !unsafe { tzinfo_acceptable(tzinfo) } {
        unsafe { set_tzinfo_type_error() };
        return ptr::null_mut();
    }
    if !unsafe { valid_fold(fold) } {
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
        (*obj).fold = fold as u8;
        (*obj).hastzinfo = own_tzinfo(&raw mut (*obj).tzinfo, tzinfo);
    }
    obj.cast::<PyObject>()
}

/// CPython _datetimemodule.c MAX_DELTA_DAYS.
const MAX_DELTA_DAYS: i64 = 999_999_999;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_delta_from_delta(
    days: c_int,
    seconds: c_int,
    useconds: c_int,
    normalize: c_int,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    // CPython new_delta_ex: normalize_d_s_us carries microseconds into seconds
    // and seconds into days (Python floor-division semantics), then
    // check_delta_day_range rejects |days| > MAX_DELTA_DAYS with OverflowError.
    // The previous body IGNORED normalize and stored the raw fields, so
    // PyDelta_FromDSU(0, 0, 1_000_000) built an invalid internal state.
    let (mut d, mut s, mut us) = (days as i64, seconds as i64, useconds as i64);
    if normalize != 0 {
        s += us.div_euclid(1_000_000);
        us = us.rem_euclid(1_000_000);
        d += s.div_euclid(86_400);
        s = s.rem_euclid(86_400);
    }
    if d.abs() > MAX_DELTA_DAYS {
        unsafe {
            set_overflow_error(c"days=...; must have magnitude <= 999999999")
        };
        return ptr::null_mut();
    }
    let typeobj = unsafe { selected_type(typeobj, &raw mut PyDateTime_DeltaType) };
    let obj = unsafe { alloc_datetime_object::<PyDateTime_Delta>(typeobj) };
    if obj.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*obj).hashcode = -1;
        (*obj).days = d as c_int;
        (*obj).seconds = s as c_int;
        (*obj).microseconds = us as c_int;
    }
    obj.cast::<PyObject>()
}

// ── timezone objects ─────────────────────────────────────────────────────────
// CPython's PyDateTime_TimeZone wraps (offset: timedelta, name: str | NULL).
// The type subclasses tzinfo so PyTZInfo_Check / check_tzinfo_subclass pass.

#[repr(C)]
struct TimeZoneObject {
    ob_base: PyObject,
    offset: *mut PyObject,
    name: *mut PyObject,
}

fn timezone_type() -> *mut PyTypeObject {
    static TIMEZONE_TYPE: once_cell::sync::Lazy<usize> = once_cell::sync::Lazy::new(|| {
        let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        ty.tp_name = c"datetime.timezone".as_ptr();
        ty.tp_basicsize = std::mem::size_of::<TimeZoneObject>() as Py_ssize_t;
        ty.tp_base = &raw mut crate::abi_types::PyDateTime_TZInfoType;
        ty.ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        ty.ob_base.ob_base.ob_refcnt = 1;
        Box::into_raw(ty) as usize
    });
    *TIMEZONE_TYPE as *mut PyTypeObject
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_timezone_from_timezone(
    offset: *mut PyObject,
    name: *mut PyObject,
) -> *mut PyObject {
    // CPython new_timezone: offset must be a timedelta with -1 day < offset <
    // 1 day; a zero offset with no name returns the UTC singleton.
    if offset.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let is_delta = unsafe {
        crate::api::typeobj::PyObject_TypeCheck(offset, &raw mut PyDateTime_DeltaType) == 1
    };
    if !is_delta {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"offset must be a timedelta".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let delta = offset.cast::<PyDateTime_Delta>();
    let (d, s, us) = unsafe { ((*delta).days, (*delta).seconds, (*delta).microseconds) };
    let total_us = (d as i64) * 86_400_000_000 + (s as i64) * 1_000_000 + us as i64;
    const DAY_US: i64 = 86_400_000_000;
    if total_us <= -DAY_US || total_us >= DAY_US {
        unsafe {
            set_value_error(
                c"offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24)",
            )
        };
        return ptr::null_mut();
    }
    let name_is_none = name.is_null() || std::ptr::eq(name, &raw mut Py_None);
    if total_us == 0 && name_is_none {
        // Reuse the UTC singleton exactly like CPython.
        let utc = &raw mut crate::abi_types::PyDateTime_TimeZone_UTC_Object;
        unsafe { crate::api::refcount::Py_INCREF(utc) };
        return utc;
    }
    unsafe { crate::api::refcount::Py_INCREF(offset) };
    let owned_name = if name_is_none {
        ptr::null_mut()
    } else {
        unsafe { crate::api::refcount::Py_INCREF(name) };
        name
    };
    let obj = Box::new(TimeZoneObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: timezone_type(),
        },
        offset,
        name: owned_name,
    });
    Box::into_raw(obj).cast::<PyObject>()
}

// ── POSIX timestamp conversion (civil-from-days, Howard Hinnant algorithm) ───
// The ABI has no host timezone database (wasm/wasi has none either), so a
// naive (tz-less) conversion decomposes in UTC — matching CPython on hosts
// with TZ unset. A tz argument that is one of OUR timezone objects applies
// its fixed offset and is attached as tzinfo.

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Split a POSIX timestamp into (whole seconds, microseconds) with CPython's
/// round-half-even microsecond rounding and carry.
fn split_timestamp(t: f64) -> Option<(i64, u32)> {
    if !t.is_finite() {
        return None;
    }
    let secs = t.floor();
    let frac = t - secs;
    let mut us = (frac * 1e6).round_ties_even() as i64;
    let mut secs = secs as i64;
    if us >= 1_000_000 {
        secs += 1;
        us -= 1_000_000;
    }
    if us < 0 {
        us = 0;
    }
    Some((secs, us as u32))
}

/// Fixed offset (in microseconds) carried by a tz argument, when it is one of
/// OUR timezone objects (or the UTC singleton). None => treat as naive/UTC.
unsafe fn tz_fixed_offset_us(tz: *mut PyObject) -> Option<i64> {
    if tz.is_null() || std::ptr::eq(tz, &raw mut Py_None) {
        return None;
    }
    if std::ptr::eq(
        tz,
        &raw mut crate::abi_types::PyDateTime_TimeZone_UTC_Object,
    ) {
        return Some(0);
    }
    if std::ptr::eq(unsafe { (*tz).ob_type }, timezone_type()) {
        let tzo = tz.cast::<TimeZoneObject>();
        let delta = unsafe { (*tzo).offset }.cast::<PyDateTime_Delta>();
        if delta.is_null() {
            return Some(0);
        }
        let (d, s, us) = unsafe { ((*delta).days, (*delta).seconds, (*delta).microseconds) };
        return Some((d as i64) * 86_400_000_000 + (s as i64) * 1_000_000 + us as i64);
    }
    // Unknown tzinfo implementation: no offset to apply at the C layer.
    None
}

unsafe fn timestamp_arg(args: *mut PyObject, index: Py_ssize_t) -> Option<f64> {
    let arg = unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
    if arg.is_null() {
        return None;
    }
    unsafe { crate::api::errors::PyErr_Clear() };
    let t = unsafe { crate::api::numbers::PyFloat_AsDouble(arg) };
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return None;
    }
    Some(t)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_datetime_from_timestamp(
    typeobj: *mut PyObject,
    args: *mut PyObject,
    _kw: *mut PyObject,
) -> *mut PyObject {
    // CPython datetime_from_timestamp(cls, (timestamp[, tz])): decompose the
    // POSIX timestamp (with microsecond rounding) and attach tz when given.
    if args.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(t) = (unsafe { timestamp_arg(args, 0) }) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"fromtimestamp() requires a numeric timestamp".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    let tz = if unsafe { crate::api::sequences::PyTuple_Size(args) } > 1 {
        unsafe { crate::api::sequences::PyTuple_GetItem(args, 1) }
    } else {
        ptr::null_mut()
    };
    let Some((mut secs, us)) = split_timestamp(t) else {
        unsafe { set_overflow_error(c"timestamp out of range for platform time") };
        return ptr::null_mut();
    };
    if let Some(offset_us) = unsafe { tz_fixed_offset_us(tz) } {
        secs += offset_us.div_euclid(1_000_000);
        // Sub-second offset components are rare (whole-minute offsets in
        // practice); fold any into the microsecond field.
        let extra_us = offset_us.rem_euclid(1_000_000);
        let total_us = us as i64 + extra_us;
        if total_us >= 1_000_000 {
            secs += 1;
        }
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(1..=9999).contains(&year) {
        unsafe { set_overflow_error(c"timestamp out of range for datetime") };
        return ptr::null_mut();
    }
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    unsafe {
        molt_cpython_abi_datetime_from_date_and_time(
            year as c_int,
            month as c_int,
            day as c_int,
            hour as c_int,
            minute as c_int,
            second as c_int,
            us as c_int,
            tz,
            typeobj.cast::<PyTypeObject>(),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_date_from_timestamp(
    typeobj: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    // CPython date_fromtimestamp(cls, (timestamp,)): decompose to a civil date.
    if args.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(t) = (unsafe { timestamp_arg(args, 0) }) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"fromtimestamp() requires a numeric timestamp".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    let Some((secs, _us)) = split_timestamp(t) else {
        unsafe { set_overflow_error(c"timestamp out of range for platform time") };
        return ptr::null_mut();
    };
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    if !(1..=9999).contains(&year) {
        unsafe { set_overflow_error(c"timestamp out of range for date") };
        return ptr::null_mut();
    }
    unsafe {
        molt_cpython_abi_date_from_date(
            year as c_int,
            month as c_int,
            day as c_int,
            typeobj.cast::<PyTypeObject>(),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_datetime_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let typeobj = (*op).ob_type;
        // Branch on a SUBTYPE walk, not exact base-type identity: a datetime/
        // time SUBCLASS instance carries ob_type != the base type, and the old
        // exact compare leaked its aware tzinfo reference on every dealloc.
        if crate::api::typeobj::PyObject_TypeCheck(op, &raw mut PyDateTime_DateTimeType) == 1 {
            let dt = op.cast::<PyDateTime_DateTime>();
            if (*dt).hastzinfo != 0 {
                crate::api::refcount::Py_XDECREF((*dt).tzinfo);
            }
        } else if crate::api::typeobj::PyObject_TypeCheck(op, &raw mut PyDateTime_TimeType) == 1 {
            let time = op.cast::<PyDateTime_Time>();
            if (*time).hastzinfo != 0 {
                crate::api::refcount::Py_XDECREF((*time).tzinfo);
            }
        } else if std::ptr::eq(typeobj, timezone_type()) {
            let tz = op.cast::<TimeZoneObject>();
            crate::api::refcount::Py_XDECREF((*tz).offset);
            crate::api::refcount::Py_XDECREF((*tz).name);
        }
        // Free through the type's own tp_free when installed (CPython:
        // Py_TYPE(self)->tp_free(self)); PyMem_Free is object's default here.
        if !typeobj.is_null()
            && let Some(tp_free) = (*typeobj).tp_free
        {
            tp_free(op.cast());
        } else {
            crate::api::memory::PyMem_Free(op.cast());
        }
    }
}

#[allow(dead_code)]
const _: Py_ssize_t = std::mem::size_of::<PyDateTime_DateTime>() as Py_ssize_t;

// ── datetime C-API capsule (`datetime.datetime_CAPI`) ────────────────────────
//
// A C extension that uses the datetime types (numpy, pandas, ...) does
// `PyDateTime_IMPORT`, which expands to
// `PyDateTimeAPI = (PyDateTime_CAPI *)PyCapsule_Import("datetime.datetime_CAPI", 0)`
// (CPython 3.12 `Include/datetime.h`). CPython's `_datetimemodule.c` publishes
// this capsule at module init via
// `PyModule_AddObject(m, "datetime_CAPI", PyCapsule_New(&CAPI, PyDateTime_CAPSULE_NAME, NULL))`.
//
// molt has no importable `datetime` C module, so before this fix
// `PyCapsule_Import` recorded a *silent failure* and returned NULL — numpy then
// returned NULL from `PyInit__multiarray_umath` (M34/M05 silent-failure class).
// We register the exact same capsule at ABI init (`molt_cpython_abi_init`), so
// `PyCapsule_Import`'s registry fast-path resolves it. Every field points at a
// molt symbol that already exists: the five static datetime type objects, the
// UTC `timezone` singleton, and the nine `molt_cpython_abi_*` constructors,
// whose signatures are byte-for-byte the CPython declarations below.

/// CPython 3.12 `PyDateTime_CAPI` (`Include/datetime.h`). The field **order and
/// count are ABI** — `PyDateTime_IMPORT` reads `PyDateTimeAPI->DateType`,
/// `->DateTimeType`, ... by fixed struct offset. Layout: 5 type objects + 1
/// singleton + 9 constructor function pointers = 15 pointer-sized fields.
#[allow(non_snake_case)]
#[repr(C)]
pub struct PyDateTime_CAPI {
    /// type objects
    pub DateType: *mut PyTypeObject,
    pub DateTimeType: *mut PyTypeObject,
    pub TimeType: *mut PyTypeObject,
    pub DeltaType: *mut PyTypeObject,
    pub TZInfoType: *mut PyTypeObject,
    /// singletons
    pub TimeZone_UTC: *mut PyObject,
    /// constructors
    pub Date_FromDate: unsafe extern "C" fn(c_int, c_int, c_int, *mut PyTypeObject) -> *mut PyObject,
    pub DateTime_FromDateAndTime: unsafe extern "C" fn(
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        *mut PyObject,
        *mut PyTypeObject,
    ) -> *mut PyObject,
    pub Time_FromTime:
        unsafe extern "C" fn(c_int, c_int, c_int, c_int, *mut PyObject, *mut PyTypeObject) -> *mut PyObject,
    pub Delta_FromDelta: unsafe extern "C" fn(c_int, c_int, c_int, c_int, *mut PyTypeObject) -> *mut PyObject,
    pub TimeZone_FromTimeZone: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject,
    /// constructors for the DB API
    pub DateTime_FromTimestamp:
        unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
    pub Date_FromTimestamp: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject,
    /// PEP 495 constructors
    pub DateTime_FromDateAndTimeAndFold: unsafe extern "C" fn(
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        *mut PyObject,
        c_int,
        *mut PyTypeObject,
    ) -> *mut PyObject,
    pub Time_FromTimeAndFold: unsafe extern "C" fn(
        c_int,
        c_int,
        c_int,
        c_int,
        *mut PyObject,
        c_int,
        *mut PyTypeObject,
    ) -> *mut PyObject,
}

/// The canonical CPython capsule name for the datetime C API
/// (`PyDateTime_CAPSULE_NAME`). Static storage so `PyCapsule_New` — which
/// stores the name pointer WITHOUT copying — retains a valid pointer forever.
const DATETIME_CAPSULE_NAME: &std::ffi::CStr = c"datetime.datetime_CAPI";

/// Assemble the `PyDateTime_CAPI` struct from molt's real datetime symbols and
/// publish it as the `datetime.datetime_CAPI` capsule, exactly like CPython's
/// `_datetimemodule.c`. Called once from the `Once`-guarded
/// `molt_cpython_abi_init`, AFTER `init_static_types` has patched the datetime
/// type objects and the UTC singleton's `ob_type`.
///
/// The `PyDateTime_CAPI` is leaked (process-lifetime singleton): numpy stores
/// `PyDateTimeAPI` pointing at it for the life of the process, matching
/// CPython, where the module owns the struct forever.
pub fn register_datetime_capi() {
    let capi = Box::new(PyDateTime_CAPI {
        DateType: &raw mut crate::abi_types::PyDateTime_DateType,
        DateTimeType: &raw mut crate::abi_types::PyDateTime_DateTimeType,
        TimeType: &raw mut crate::abi_types::PyDateTime_TimeType,
        DeltaType: &raw mut crate::abi_types::PyDateTime_DeltaType,
        TZInfoType: &raw mut crate::abi_types::PyDateTime_TZInfoType,
        TimeZone_UTC: &raw mut crate::abi_types::PyDateTime_TimeZone_UTC_Object,
        Date_FromDate: molt_cpython_abi_date_from_date,
        DateTime_FromDateAndTime: molt_cpython_abi_datetime_from_date_and_time,
        Time_FromTime: molt_cpython_abi_time_from_time,
        Delta_FromDelta: molt_cpython_abi_delta_from_delta,
        TimeZone_FromTimeZone: molt_cpython_abi_timezone_from_timezone,
        DateTime_FromTimestamp: molt_cpython_abi_datetime_from_timestamp,
        Date_FromTimestamp: molt_cpython_abi_date_from_timestamp,
        DateTime_FromDateAndTimeAndFold: molt_cpython_abi_datetime_from_date_and_time_and_fold,
        Time_FromTimeAndFold: molt_cpython_abi_time_from_time_and_fold,
    });
    let capi_ptr = Box::into_raw(capi).cast::<std::ffi::c_void>();
    // `PyCapsule_New` inserts into the capsule registry (so `PyCapsule_Import`'s
    // fast path resolves it) and into the object bridge. The returned capsule
    // object is intentionally never DECREF'd — like CPython's module-owned
    // capsule it lives for the process lifetime.
    let capsule = unsafe {
        crate::api::capsule::PyCapsule_New(capi_ptr, DATETIME_CAPSULE_NAME.as_ptr(), None)
    };
    debug_assert!(!capsule.is_null(), "datetime CAPI capsule registration failed");
}

#[cfg(test)]
mod capi_tests {
    use super::*;

    /// The struct is exactly 15 pointer-sized fields (5 types + 1 singleton + 9
    /// constructors). A drift here silently misaligns every field numpy reads by
    /// offset — the whole point of pinning the CPython 3.12 layout.
    #[test]
    fn datetime_capi_has_exact_field_count() {
        assert_eq!(
            std::mem::size_of::<PyDateTime_CAPI>(),
            15 * std::mem::size_of::<*mut std::ffi::c_void>(),
            "PyDateTime_CAPI must be 15 pointer-sized fields (CPython 3.12 layout)"
        );
    }

    /// After ABI init the `datetime.datetime_CAPI` capsule is importable and its
    /// fields resolve to molt's real datetime symbols — the roundtrip numpy's
    /// `PyDateTime_IMPORT` performs.
    #[test]
    fn datetime_capi_capsule_roundtrips() {
        crate::bridge::molt_cpython_abi_init();
        let ptr = unsafe {
            crate::api::capsule::PyCapsule_Import(DATETIME_CAPSULE_NAME.as_ptr(), 0)
        };
        assert!(!ptr.is_null(), "PyCapsule_Import(datetime.datetime_CAPI) returned NULL");
        let capi = ptr.cast::<PyDateTime_CAPI>();
        unsafe {
            assert!(
                std::ptr::eq((*capi).DateType, &raw mut crate::abi_types::PyDateTime_DateType),
                "DateType must alias molt's PyDateTime_DateType"
            );
            assert!(
                std::ptr::eq(
                    (*capi).DateTimeType,
                    &raw mut crate::abi_types::PyDateTime_DateTimeType
                ),
                "DateTimeType must alias molt's PyDateTime_DateTimeType"
            );
            assert!(
                std::ptr::eq(
                    (*capi).TZInfoType,
                    &raw mut crate::abi_types::PyDateTime_TZInfoType
                ),
                "TZInfoType must alias molt's PyDateTime_TZInfoType"
            );
            assert!(
                std::ptr::eq(
                    (*capi).TimeZone_UTC,
                    &raw mut crate::abi_types::PyDateTime_TimeZone_UTC_Object
                ),
                "TimeZone_UTC must alias molt's UTC singleton"
            );
        }
        // The constructor pointers must be non-null and callable (build a date).
        let date = unsafe {
            ((*capi).Date_FromDate)(2026, 7, 10, std::ptr::null_mut())
        };
        assert!(!date.is_null(), "CAPI Date_FromDate produced NULL for a valid date");
        unsafe { crate::api::refcount::Py_DECREF(date) };
    }
}
