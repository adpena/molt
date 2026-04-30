// System, GC, time, signal, traceback, profiling, and related runtime support.
// Split from ops.rs for compilation-unit size reduction.

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::object::ops_string::wtf8_codepoint_at;
use crate::state::runtime_state::PythonVersionInfo;
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::CStr;
use std::io::{BufRead, BufReader};
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};
// Vector aggregate operations (molt_vec_*) live in ops_vec.rs.
pub(crate) enum SliceError {
    Type,
    Value,
}

pub(crate) fn slice_error(_py: &PyToken<'_>, err: SliceError) -> u64 {
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    match err {
        SliceError::Type => raise_exception::<_>(
            _py,
            "TypeError",
            "slice indices must be integers or None or have an __index__ method",
        ),
        SliceError::Value => raise_exception::<_>(_py, "ValueError", "slice step cannot be zero"),
    }
}

pub(crate) fn decode_slice_bound(
    _py: &PyToken<'_>,
    obj: MoltObject,
    len: isize,
    default: isize,
) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(mut idx) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    let len_big = BigInt::from(len);
    if idx.is_negative() {
        idx += &len_big;
    }
    if idx < BigInt::zero() {
        return Ok(0);
    }
    if idx > len_big {
        return Ok(len);
    }
    Ok(idx.to_isize().unwrap_or(len))
}

pub(crate) fn decode_slice_bound_neg(
    _py: &PyToken<'_>,
    obj: MoltObject,
    len: isize,
    default: isize,
) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(mut idx) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    let len_big = BigInt::from(len);
    if idx.is_negative() {
        idx += &len_big;
    }
    let neg_one = BigInt::from(-1);
    if idx < neg_one {
        return Ok(-1);
    }
    if idx >= len_big {
        return Ok(len - 1);
    }
    Ok(idx.to_isize().unwrap_or(len - 1))
}

pub(crate) fn decode_slice_step(_py: &PyToken<'_>, obj: MoltObject) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(1);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(step) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    if step.is_zero() {
        return Err(SliceError::Value);
    }
    if let Some(step) = step.to_i64() {
        return Ok(step as isize);
    }
    if step.is_negative() {
        return Ok(-(i64::MAX as isize));
    }
    Ok(i64::MAX as isize)
}

pub(crate) fn normalize_slice_indices(
    _py: &PyToken<'_>,
    len: isize,
    start_obj: MoltObject,
    stop_obj: MoltObject,
    step_obj: MoltObject,
) -> Result<(isize, isize, isize), SliceError> {
    let step = decode_slice_step(_py, step_obj)?;
    if step > 0 {
        let start = decode_slice_bound(_py, start_obj, len, 0)?;
        let stop = decode_slice_bound(_py, stop_obj, len, len)?;
        return Ok((start, stop, step));
    }
    let start_default = if len == 0 { -1 } else { len - 1 };
    let stop_default = -1;
    let start = decode_slice_bound_neg(_py, start_obj, len, start_default)?;
    let stop = decode_slice_bound_neg(_py, stop_obj, len, stop_default)?;
    Ok((start, stop, step))
}

pub(crate) fn collect_slice_indices(start: isize, stop: isize, step: isize) -> Vec<usize> {
    let mut out = Vec::new();
    if step > 0 {
        let mut i = start;
        while i < stop {
            out.push(i as usize);
            let Some(next) = i.checked_add(step) else {
                break;
            };
            i = next;
        }
    } else {
        let mut i = start;
        while i > stop {
            out.push(i as usize);
            let Some(next) = i.checked_add(step) else {
                break;
            };
            i = next;
        }
    }
    out
}

pub(crate) fn collect_iterable_values(
    _py: &PyToken<'_>,
    bits: u64,
    err_msg: &str,
) -> Option<Vec<u64>> {
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        if exception_pending(_py) {
            return None;
        }
        return raise_exception::<_>(_py, "TypeError", err_msg);
    }
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending(_py) {
            return None;
        }
        let pair_ptr = obj_from_bits(pair_bits).as_ptr()?;
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return None;
            }
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            out.push(elems[0]);
        }
    }
    Some(out)
}

pub(crate) fn ord_length_error(_py: &PyToken<'_>, len: usize) -> u64 {
    let msg = format!("ord() expected a character, but string of length {len} found");
    raise_exception::<_>(_py, "TypeError", &msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ord(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let char_count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    if char_count != 1 {
                        return ord_length_error(_py, char_count as usize);
                    }
                    let Some(code) = wtf8_codepoint_at(bytes, 0) else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(code.to_u32() as i64).bits();
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    if len != 1 {
                        return ord_length_error(_py, len);
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    return MoltObject::from_int(bytes[0] as i64).bits();
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("ord() expected string of length 1, but {type_name} found");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[derive(Clone, Copy)]
pub(crate) struct GcState {
    pub(crate) enabled: bool,
    pub(crate) thresholds: (i64, i64, i64),
    pub(crate) debug_flags: i64,
    pub(crate) count: (i64, i64, i64),
}

pub(crate) fn gc_state() -> &'static Mutex<GcState> {
    static GC_STATE: OnceLock<Mutex<GcState>> = OnceLock::new();
    GC_STATE.get_or_init(|| {
        Mutex::new(GcState {
            enabled: true,
            thresholds: (0, 0, 0),
            debug_flags: 0,
            count: (0, 0, 0),
        })
    })
}

pub(crate) fn gc_int_arg(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<i64, u64> {
    if let Some(value) = to_i64(obj_from_bits(bits)) {
        return Ok(value);
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(bits) {
        let big = unsafe { bigint_ref(big_ptr) };
        let Some(value) = big.to_i64() else {
            let msg = format!("{label} value out of range");
            return Err(raise_exception::<_>(_py, "OverflowError", &msg));
        };
        return Ok(value);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

pub(crate) fn trace_sys_version() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SYS_VERSION").as_deref() == Ok("1"))
}

pub(crate) fn env_sys_version_info() -> Option<PythonVersionInfo> {
    let raw = std::env::var("MOLT_SYS_VERSION_INFO").ok()?;
    if trace_sys_version() {
        eprintln!("molt sys version: env raw={raw}");
    }
    let mut parts = raw.split(',');
    let major = parts.next()?.trim().parse::<i64>().ok()?;
    let minor = parts.next()?.trim().parse::<i64>().ok()?;
    let micro = parts.next()?.trim().parse::<i64>().ok()?;
    let releaselevel = parts.next()?.trim().to_string();
    let serial = parts.next()?.trim().parse::<i64>().ok()?;
    if major < 0 || minor < 0 || micro < 0 || serial < 0 {
        return None;
    }
    if releaselevel.is_empty() {
        return None;
    }
    let info = PythonVersionInfo {
        major,
        minor,
        micro,
        releaselevel,
        serial,
    };
    if trace_sys_version() {
        eprintln!(
            "molt sys version: parsed {}.{}.{} {} {}",
            info.major, info.minor, info.micro, info.releaselevel, info.serial
        );
    }
    Some(info)
}

pub(crate) fn env_target_python_info() -> Option<PythonVersionInfo> {
    if let Some(info) = env_sys_version_info() {
        return Some(info);
    }
    let raw = std::env::var("MOLT_PYTHON_VERSION").ok()?;
    let mut parts = raw.trim().split('.');
    let major = parts.next()?.trim().parse::<i64>().ok()?;
    let minor = parts.next()?.trim().parse::<i64>().ok()?;
    let micro = parts
        .next()
        .and_then(|part| part.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if major < 0 || minor < 0 || micro < 0 {
        return None;
    }
    Some(PythonVersionInfo {
        major,
        minor,
        micro,
        releaselevel: "final".to_string(),
        serial: 0,
    })
}

pub(crate) fn default_sys_version_info() -> PythonVersionInfo {
    env_target_python_info().unwrap_or_else(|| PythonVersionInfo {
        major: 3,
        minor: 12,
        micro: 0,
        releaselevel: "final".to_string(),
        serial: 0,
    })
}

fn push_i64_decimal(out: &mut String, value: i64) {
    if value == 0 {
        out.push('0');
        return;
    }
    let negative = value < 0;
    let mut n = value.unsigned_abs();
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while n > 0 {
        digits[len] = (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    if negative {
        out.push('-');
    }
    while len > 0 {
        len -= 1;
        out.push(char::from(b'0' + digits[len]));
    }
}

pub(crate) fn format_sys_version(info: &PythonVersionInfo) -> String {
    let mut out = String::with_capacity(48);
    push_i64_decimal(&mut out, info.major);
    out.push('.');
    push_i64_decimal(&mut out, info.minor);
    out.push('.');
    push_i64_decimal(&mut out, info.micro);
    match info.releaselevel.as_str() {
        "alpha" => {
            out.push('a');
            push_i64_decimal(&mut out, info.serial);
        }
        "beta" => {
            out.push('b');
            push_i64_decimal(&mut out, info.serial);
        }
        "candidate" => {
            out.push_str("rc");
            push_i64_decimal(&mut out, info.serial);
        }
        "final" => {}
        other => {
            out.push_str(other);
            push_i64_decimal(&mut out, info.serial);
        }
    }
    out.push_str(" (molt)");
    out
}

pub(crate) const DEFAULT_SYS_API_VERSION: i64 = 1013;
pub(crate) const SYS_HEX_RELEASELEVEL_ALPHA: i64 = 0xA;
pub(crate) const SYS_HEX_RELEASELEVEL_BETA: i64 = 0xB;
pub(crate) const SYS_HEX_RELEASELEVEL_CANDIDATE: i64 = 0xC;
pub(crate) const SYS_HEX_RELEASELEVEL_FINAL: i64 = 0xF;

pub(crate) fn releaselevel_hex_nibble(releaselevel: &str) -> i64 {
    match releaselevel {
        "alpha" => SYS_HEX_RELEASELEVEL_ALPHA,
        "beta" => SYS_HEX_RELEASELEVEL_BETA,
        "candidate" | "rc" => SYS_HEX_RELEASELEVEL_CANDIDATE,
        "final" => SYS_HEX_RELEASELEVEL_FINAL,
        _ => SYS_HEX_RELEASELEVEL_FINAL,
    }
}

pub(crate) fn sys_hexversion_from_info(info: &PythonVersionInfo) -> i64 {
    let major = (info.major & 0xFF) << 24;
    let minor = (info.minor & 0xFF) << 16;
    let micro = (info.micro & 0xFF) << 8;
    let releaselevel = releaselevel_hex_nibble(&info.releaselevel) << 4;
    let serial = info.serial & 0xF;
    major | minor | micro | releaselevel | serial
}

pub(crate) fn sys_api_version() -> i64 {
    std::env::var("MOLT_SYS_API_VERSION")
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_SYS_API_VERSION)
}

pub(crate) fn sys_abiflags() -> String {
    std::env::var("MOLT_SYS_ABIFLAGS").unwrap_or_default()
}

pub(crate) fn sys_implementation_name() -> String {
    match std::env::var("MOLT_SYS_IMPLEMENTATION_NAME") {
        Ok(raw) if !raw.trim().is_empty() => raw,
        _ => "molt".to_string(),
    }
}

pub(crate) fn sys_cache_tag(name: &str, info: &PythonVersionInfo) -> String {
    match std::env::var("MOLT_SYS_CACHE_TAG") {
        Ok(raw) if !raw.is_empty() => raw,
        _ => format!("{name}-{}{}", info.major, info.minor),
    }
}

pub(crate) const DEFAULT_SYS_FLAGS_INT_MAX_STR_DIGITS: i64 = 0;

pub(crate) fn env_flag_level(var: &str) -> Option<i64> {
    let raw = std::env::var(var).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(1);
    }
    match trimmed.parse::<i64>() {
        Ok(value) if value > 0 => Some(value),
        Ok(_) => Some(0),
        Err(_) => Some(1),
    }
}

pub(crate) fn env_flag_bool(var: &str) -> Option<i64> {
    env_flag_level(var).map(|value| if value == 0 { 0 } else { 1 })
}

pub(crate) fn env_non_negative_i64(var: &str) -> Option<i64> {
    std::env::var(var)
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
}

pub(crate) fn sys_flags_hash_randomization() -> i64 {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
                return 1;
            }
            let seed: u32 = value.parse().unwrap_or_else(|_| fatal_hash_seed(&value));
            if seed == 0 { 0 } else { 1 }
        }
        Err(_) => 1,
    }
}

pub(crate) fn current_sys_version_info(state: &RuntimeState) -> (PythonVersionInfo, bool) {
    let mut guard = state.sys_version_info.lock().unwrap();
    if let Some(existing) = guard.as_ref() {
        (existing.clone(), false)
    } else {
        let init = default_sys_version_info();
        *guard = Some(init.clone());
        (init, true)
    }
}

pub(crate) fn runtime_target_python_info(state: &RuntimeState) -> PythonVersionInfo {
    current_sys_version_info(state).0
}

pub(crate) fn runtime_target_minor(_py: &PyToken<'_>) -> i64 {
    runtime_target_python_info(runtime_state(_py)).minor
}

pub(crate) fn runtime_target_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    let info = runtime_target_python_info(runtime_state(_py));
    info.major > major || (info.major == major && info.minor >= minor)
}

pub(crate) fn alloc_sys_version_info_tuple(
    _py: &PyToken<'_>,
    info: &PythonVersionInfo,
) -> Option<u64> {
    let release_ptr = alloc_string(_py, info.releaselevel.as_bytes());
    if release_ptr.is_null() {
        return None;
    }
    let release_bits = MoltObject::from_ptr(release_ptr).bits();
    let elems = [
        MoltObject::from_int(info.major).bits(),
        MoltObject::from_int(info.minor).bits(),
        MoltObject::from_int(info.micro).bits(),
        release_bits,
        MoltObject::from_int(info.serial).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, release_bits);
        return None;
    }
    for bits in elems {
        dec_ref_bits(_py, bits);
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

pub(crate) fn dict_set_bytes_key(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key: &[u8],
    value_bits: u64,
) -> bool {
    let key_ptr = alloc_string(_py, key);
    if key_ptr.is_null() {
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    }
    dec_ref_bits(_py, key_bits);
    true
}

// molt_set_argv, molt_set_argv_utf16 live in ops.rs

#[cfg(all(not(target_arch = "wasm32"), unix))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return Err("process time before epoch".to_string());
    }
    Ok(std::time::Duration::new(
        ts.tv_sec as u64,
        ts.tv_nsec as u32,
    ))
}

#[cfg(all(not(target_arch = "wasm32"), windows))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let handle = unsafe { GetCurrentProcess() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let kernel_100ns = ((kernel.dwHighDateTime as u64) << 32) | kernel.dwLowDateTime as u64;
    let user_100ns = ((user.dwHighDateTime as u64) << 32) | user.dwLowDateTime as u64;
    let total_100ns = kernel_100ns.saturating_add(user_100ns);
    let secs = total_100ns / 10_000_000;
    let nanos = (total_100ns % 10_000_000) * 100;
    Ok(std::time::Duration::new(secs, nanos as u32))
}

#[cfg(any(target_arch = "wasm32", not(any(unix, windows))))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    Err("process_time unavailable".to_string())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeParts {
    pub(crate) year: i32,
    pub(crate) month: i32,
    pub(crate) day: i32,
    pub(crate) hour: i32,
    pub(crate) minute: i32,
    pub(crate) second: i32,
    pub(crate) wday: i32,
    pub(crate) yday: i32,
    pub(crate) isdst: i32,
}

pub(crate) fn time_parts_to_tuple(_py: &PyToken<'_>, parts: TimeParts) -> u64 {
    let elems = [
        MoltObject::from_int(parts.year as i64).bits(),
        MoltObject::from_int(parts.month as i64).bits(),
        MoltObject::from_int(parts.day as i64).bits(),
        MoltObject::from_int(parts.hour as i64).bits(),
        MoltObject::from_int(parts.minute as i64).bits(),
        MoltObject::from_int(parts.second as i64).bits(),
        MoltObject::from_int(parts.wday as i64).bits(),
        MoltObject::from_int(parts.yday as i64).bits(),
        MoltObject::from_int(parts.isdst as i64).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn time_parts_from_tm(tm: &libc::tm) -> TimeParts {
    let wday = (tm.tm_wday + 6).rem_euclid(7);
    TimeParts {
        year: tm.tm_year + 1900,
        month: tm.tm_mon + 1,
        day: tm.tm_mday,
        hour: tm.tm_hour,
        minute: tm.tm_min,
        second: tm.tm_sec,
        wday,
        yday: tm.tm_yday + 1,
        isdst: tm.tm_isdst,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tm_from_time_parts(_py: &PyToken<'_>, parts: TimeParts) -> Result<libc::tm, u64> {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    if tm.tm_mon < 0 || tm.tm_mon > 11 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "strftime() argument 2 out of range",
        ));
    }
    Ok(tm)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn day_of_year(year: i32, month: i32, day: i32) -> i32 {
    const DAYS_BEFORE_MONTH: [[i32; 13]; 2] = [
        [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334],
        [0, 0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335],
    ];
    let leap = if is_leap_year(year) { 1 } else { 0 };
    let m = month.clamp(1, 12) as usize;
    DAYS_BEFORE_MONTH[leap][m] + day
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = (yoe + era * 400) as i32;
    let doy = (doe - (365 * yoe + yoe / 4 - yoe / 100)) as i32;
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1);
    let m = (mp + if mp < 10 { 3 } else { -9 });
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn time_parts_from_epoch_utc(secs: i64) -> TimeParts {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as i32;
    let minute = ((rem % 3600) / 60) as i32;
    let second = (rem % 60) as i32;
    let (year, month, day) = civil_from_days(days);
    let yday = day_of_year(year, month, day);
    let wday = ((days + 3).rem_euclid(7)) as i32;
    TimeParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        wday,
        yday,
        isdst: 0,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn timezone_west_wasm() -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_timezone_host() };
    if offset == i64::MIN {
        return Err("timezone unavailable".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn local_offset_west_wasm(secs: i64) -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_local_offset_host(secs) };
    if offset == i64::MIN {
        return Err("localtime failed".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn tzname_label_wasm(which: i32) -> Result<String, String> {
    let mut buf = vec![0u8; 256];
    let mut out_len: u32 = 0;
    let status = unsafe {
        crate::molt_time_tzname_host(
            which,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            (&mut out_len as *mut u32) as u32,
        )
    };
    if status != 0 {
        return Err("tzname unavailable".to_string());
    }
    let out_len = usize::try_from(out_len).map_err(|_| "tzname unavailable".to_string())?;
    if out_len > buf.len() {
        return Err("tzname unavailable".to_string());
    }
    buf.truncate(out_len);
    String::from_utf8(buf).map_err(|_| "tzname unavailable".to_string())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn tzname_wasm() -> Result<(String, String), String> {
    let std_name = tzname_label_wasm(0)?;
    let dst_name = tzname_label_wasm(1)?;
    Ok((std_name, dst_name))
}

pub(crate) fn current_epoch_secs_i64() -> Result<i64, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time before epoch".to_string())?;
    Ok(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
}

pub(crate) fn parse_time_seconds(_py: &PyToken<'_>, secs_bits: u64) -> Result<i64, u64> {
    let obj = obj_from_bits(secs_bits);
    if obj.is_none() {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return Err(raise_exception::<_>(
                    _py,
                    "OSError",
                    "system time before epoch",
                ));
            }
        };
        let secs = now.as_secs();
        let secs = i64::try_from(secs).unwrap_or(i64::MAX);
        return Ok(secs);
    }
    let Some(val) = to_f64(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, secs_bits));
        let msg = format!("an integer is required (got type {type_name})");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    if !val.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    let secs = val.trunc();
    let (min, max) = time_t_bounds();
    if secs < min as f64 || secs > max as f64 {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    Ok(secs as i64)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn time_t_bounds() -> (i128, i128) {
    let size = std::mem::size_of::<libc::time_t>();
    if size == 4 {
        (i32::MIN as i128, i32::MAX as i128)
    } else {
        (i64::MIN as i128, i64::MAX as i128)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn time_t_bounds() -> (i128, i128) {
    (i64::MIN as i128, i64::MAX as i128)
}

pub(crate) fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let mut y = year as i64;
    let m = month as i64;
    let d = day as i64;
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tm_to_epoch_seconds(tm: &libc::tm) -> i64 {
    let year = tm.tm_year + 1900;
    let month = tm.tm_mon + 1;
    let day = tm.tm_mday;
    let days = days_from_civil(year, month, day);
    let seconds = (tm.tm_hour as i64) * 3600 + (tm.tm_min as i64) * 60 + (tm.tm_sec as i64);
    days.saturating_mul(86_400).saturating_add(seconds)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn offset_west_from_secs(secs: i64) -> Result<i64, String> {
    let secs = secs as libc::time_t;
    let local_tm = localtime_tm(secs)?;
    let utc_tm = gmtime_tm(secs)?;
    let local_secs = tm_to_epoch_seconds(&local_tm);
    let utc_secs = tm_to_epoch_seconds(&utc_tm);
    Ok(utc_secs.saturating_sub(local_secs))
}

pub(crate) fn parse_time_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "strftime() argument 2 must be tuple",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            let type_name = class_name_for_error(type_of_bits(_py, tuple_bits));
            let msg = format!("strftime() argument 2 must be tuple, not {type_name}");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "time tuple must have exactly 9 elements",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "strftime() argument 2 out of range",
                ));
            }
            *slot = val;
        }
        let year = vals[0] as i32;
        let month = vals[1] as i32;
        let day = vals[2] as i32;
        let hour = vals[3] as i32;
        let minute = vals[4] as i32;
        let second = vals[5] as i32;
        let wday = vals[6] as i32;
        let yday = vals[7] as i32;
        let isdst = vals[8] as i32;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..=23).contains(&hour)
            || !(0..=59).contains(&minute)
            || !(0..=60).contains(&second)
            || !(0..=6).contains(&wday)
            || !(1..=366).contains(&yday)
        {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        if ![-1, 0, 1].contains(&isdst) {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        Ok(TimeParts {
            year,
            month,
            day,
            hour,
            minute,
            second,
            wday,
            yday,
            isdst,
        })
    }
}

pub(crate) fn asctime_from_parts(parts: TimeParts) -> Result<String, String> {
    const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTH_ABBR: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !(0..=6).contains(&parts.wday)
        || !(1..=12).contains(&parts.month)
        || !(1..=31).contains(&parts.day)
    {
        return Err("time tuple elements out of range".to_string());
    }
    let wday = WEEKDAY_ABBR[parts.wday as usize];
    let month = MONTH_ABBR[(parts.month - 1) as usize];
    Ok(format!(
        "{wday} {month} {:2} {:02}:{:02}:{:02} {:04}",
        parts.day, parts.hour, parts.minute, parts.second, parts.year
    ))
}

pub(crate) fn parse_mktime_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "mktime(): illegal time tuple argument",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "mktime(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok(TimeParts {
            year: vals[0] as i32,
            month: vals[1] as i32,
            day: vals[2] as i32,
            hour: vals[3] as i32,
            minute: vals[4] as i32,
            second: vals[5] as i32,
            wday: vals[6] as i32,
            yday: vals[7] as i32,
            isdst: vals[8] as i32,
        })
    }
}

pub(crate) fn parse_timegm_tuple(
    _py: &PyToken<'_>,
    tuple_bits: u64,
) -> Result<(i32, i32, i32, i32, i32, i32), u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 6 {
            let msg = format!(
                "not enough values to unpack (expected 6, got {})",
                elems.len()
            );
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        }
        let mut vals = [0i64; 6];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "timegm(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok((
            vals[0] as i32,
            vals[1] as i32,
            vals[2] as i32,
            vals[3] as i32,
            vals[4] as i32,
            vals[5] as i32,
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn localtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::localtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gmtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::gmtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::gmtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn strftime_wasm(format: &str, parts: TimeParts) -> Result<String, String> {
    const WEEKDAY_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const WEEKDAY_LONG: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    const MONTH_SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const MONTH_LONG: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    fn push_num(out: &mut String, val: i32, width: usize, pad: char) {
        let mut buf = [pad as u8; 12];
        let mut idx = buf.len();
        let mut n = val.unsigned_abs();
        if n == 0 {
            idx -= 1;
            buf[idx] = b'0';
        } else {
            while n > 0 {
                let digit = (n % 10) as u8;
                idx -= 1;
                buf[idx] = b'0' + digit;
                n /= 10;
            }
        }
        let len = buf.len() - idx;
        let needed = width.saturating_sub(len + if val < 0 { 1 } else { 0 });
        for _ in 0..needed {
            out.push(pad);
        }
        if val < 0 {
            out.push('-');
        }
        out.push_str(std::str::from_utf8(&buf[idx..]).unwrap_or("0"));
    }

    fn jan1_wday_mon0(yday: i32, wday_mon0: i32) -> i32 {
        let offset = (yday - 1).rem_euclid(7);
        (wday_mon0 - offset).rem_euclid(7)
    }

    fn week_number_sun(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_sun0 = (jan1_wday_mon0 + 1).rem_euclid(7);
        let first_sunday = 1 + (7 - jan1_sun0).rem_euclid(7);
        if yday < first_sunday {
            0
        } else {
            1 + (yday - first_sunday) / 7
        }
    }

    fn week_number_mon(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let first_monday = 1 + (7 - jan1_wday_mon0).rem_euclid(7);
        if yday < first_monday {
            0
        } else {
            1 + (yday - first_monday) / 7
        }
    }

    fn weeks_in_year(year: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_mon1 = jan1_wday_mon0 + 1;
        if jan1_mon1 == 4 || (is_leap_year(year) && jan1_mon1 == 3) {
            53
        } else {
            52
        }
    }

    fn iso_week_date(year: i32, yday: i32, wday_mon0: i32) -> (i32, i32, i32) {
        let weekday = wday_mon0 + 1;
        let mut week = (yday - weekday + 10) / 7;
        let jan1_wday = jan1_wday_mon0(yday, wday_mon0);
        let mut iso_year = year;
        let max_week = weeks_in_year(year, jan1_wday);
        if week < 1 {
            iso_year -= 1;
            let prev_days = if is_leap_year(iso_year) { 366 } else { 365 };
            let prev_jan1 = (jan1_wday - (prev_days % 7)).rem_euclid(7);
            week = weeks_in_year(iso_year, prev_jan1);
        } else if week > max_week {
            iso_year += 1;
            week = 1;
        }
        (iso_year, week, weekday)
    }

    let mut out = String::with_capacity(format.len() + 16);
    let mut iter = format.chars();
    while let Some(ch) = iter.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = iter.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'a' => out.push_str(WEEKDAY_SHORT[parts.wday as usize]),
            'A' => out.push_str(WEEKDAY_LONG[parts.wday as usize]),
            'b' | 'h' => out.push_str(MONTH_SHORT[(parts.month - 1) as usize]),
            'B' => out.push_str(MONTH_LONG[(parts.month - 1) as usize]),
            'C' => {
                let century = parts.year.div_euclid(100);
                push_num(&mut out, century, 2, '0');
            }
            'd' => push_num(&mut out, parts.day, 2, '0'),
            'e' => push_num(&mut out, parts.day, 2, ' '),
            'H' => push_num(&mut out, parts.hour, 2, '0'),
            'I' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
            }
            'k' => push_num(&mut out, parts.hour, 2, ' '),
            'l' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, ' ');
            }
            'j' => push_num(&mut out, parts.yday, 3, '0'),
            'm' => push_num(&mut out, parts.month, 2, '0'),
            'M' => push_num(&mut out, parts.minute, 2, '0'),
            'p' => out.push_str(if parts.hour < 12 { "AM" } else { "PM" }),
            'S' => push_num(&mut out, parts.second, 2, '0'),
            'U' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_sun(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'W' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_mon(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'w' => {
                let wday_sun0 = (parts.wday + 1).rem_euclid(7);
                push_num(&mut out, wday_sun0, 1, '0');
            }
            'u' => {
                let wday_mon1 = parts.wday + 1;
                push_num(&mut out, wday_mon1, 1, '0');
            }
            'x' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'X' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'y' => {
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'Y' => push_num(&mut out, parts.year, 4, '0'),
            'Z' => out.push_str("UTC"),
            'z' => out.push_str("+0000"),
            'c' => {
                out.push_str(WEEKDAY_SHORT[parts.wday as usize]);
                out.push(' ');
                out.push_str(MONTH_SHORT[(parts.month - 1) as usize]);
                out.push(' ');
                push_num(&mut out, parts.day, 2, ' ');
                out.push(' ');
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                push_num(&mut out, parts.year, 4, '0');
            }
            'D' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'F' => {
                push_num(&mut out, parts.year, 4, '0');
                out.push('-');
                push_num(&mut out, parts.month, 2, '0');
                out.push('-');
                push_num(&mut out, parts.day, 2, '0');
            }
            'R' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
            }
            'r' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                out.push_str(if parts.hour < 12 { "AM" } else { "PM" });
            }
            'T' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'G' | 'g' | 'V' => {
                let (iso_year, iso_week, _) = iso_week_date(parts.year, parts.yday, parts.wday);
                match spec {
                    'G' => push_num(&mut out, iso_year, 4, '0'),
                    'g' => {
                        let yy = iso_year.rem_euclid(100);
                        push_num(&mut out, yy, 2, '0');
                    }
                    _ => push_num(&mut out, iso_week, 2, '0'),
                }
            }
            _ => {
                return Err(format!("unsupported strftime directive %{spec}"));
            }
        }
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tzname_native() -> Result<(String, String), String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut tzname: [*mut libc::c_char; 2];
        }
        tzset();
        let std_ptr = tzname[0];
        let dst_ptr = tzname[1];
        if std_ptr.is_null() || dst_ptr.is_null() {
            return Err("tzname unavailable".to_string());
        }
        let std_name = CStr::from_ptr(std_ptr).to_string_lossy().into_owned();
        let dst_name = CStr::from_ptr(dst_ptr).to_string_lossy().into_owned();
        Ok((std_name, dst_name))
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("tzname unavailable".to_string());
        }
        let std_len = info
            .StandardName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.StandardName.len());
        let dst_len = info
            .DaylightName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.DaylightName.len());
        let std_name = String::from_utf16_lossy(&info.StandardName[..std_len]);
        let dst_name = String::from_utf16_lossy(&info.DaylightName[..dst_len]);
        return Ok((std_name, dst_name));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn timezone_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut timezone: libc::c_long;
        }
        tzset();
        Ok(timezone)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("timezone unavailable".to_string());
        }
        let bias = info.Bias + info.StandardBias;
        return Ok((bias as i64) * 60);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn daylight_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut daylight: libc::c_int;
        }
        tzset();
        Ok(if daylight != 0 { 1 } else { 0 })
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("daylight unavailable".to_string());
        }
        return Ok(if info.DaylightDate.wMonth != 0 { 1 } else { 0 });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sample_offset_west_native(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    offset_west_from_secs(secs)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn altzone_native() -> Result<i64, String> {
    let std_offset = timezone_native()?;
    if daylight_native()? == 0 {
        return Ok(std_offset);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("altzone unavailable".to_string());
        }
        let bias = info.Bias + info.DaylightBias;
        return Ok((bias as i64) * 60);
    }
    #[cfg(unix)]
    {
        let now = current_epoch_secs_i64()?;
        let local_tm = localtime_tm(now as libc::time_t)?;
        let year = local_tm.tm_year + 1900;
        let jan = sample_offset_west_native(year, 1, 1).unwrap_or(std_offset);
        let jul = sample_offset_west_native(year, 7, 1).unwrap_or(std_offset);
        if jan != std_offset && jul == std_offset {
            return Ok(jan);
        }
        if jul != std_offset && jan == std_offset {
            return Ok(jul);
        }
        if jan != jul {
            return Ok(std::cmp::min(jan, jul));
        }
        Ok(jan)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sample_offset_west_wasm(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    local_offset_west_wasm(secs)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn daylight_wasm() -> Result<i64, String> {
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1)?;
    let jul = sample_offset_west_wasm(year, 7, 1)?;
    Ok(if jan != jul { 1 } else { 0 })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn altzone_wasm() -> Result<i64, String> {
    let std_offset = timezone_west_wasm()?;
    if daylight_wasm()? == 0 {
        return Ok(std_offset);
    }
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1).unwrap_or(std_offset);
    let jul = sample_offset_west_wasm(year, 7, 1).unwrap_or(std_offset);
    if jan != std_offset && jul == std_offset {
        return Ok(jan);
    }
    if jul != std_offset && jan == std_offset {
        return Ok(jul);
    }
    if jan != jul {
        return Ok(std::cmp::min(jan, jul));
    }
    Ok(jan)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn mktime_native(parts: TimeParts) -> f64 {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    let out = unsafe { libc::mktime(&mut tm as *mut libc::tm) };
    out as f64
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn mktime_wasm(parts: TimeParts) -> Result<f64, String> {
    let days = days_from_civil(parts.year, parts.month, parts.day);
    let local_secs = days
        .saturating_mul(86_400)
        .saturating_add((parts.hour as i64).saturating_mul(3600))
        .saturating_add((parts.minute as i64).saturating_mul(60))
        .saturating_add(parts.second as i64);
    let std_offset = timezone_west_wasm()?;
    let utc_secs = if parts.isdst > 0 {
        let dst_offset = altzone_wasm().unwrap_or(std_offset);
        local_secs.saturating_add(dst_offset)
    } else if parts.isdst == 0 {
        local_secs.saturating_add(std_offset)
    } else {
        let mut guess = local_secs.saturating_add(std_offset);
        for _ in 0..3 {
            let offset = local_offset_west_wasm(guess).unwrap_or(std_offset);
            let next = local_secs.saturating_add(offset);
            if next == guess {
                break;
            }
            guess = next;
        }
        guess
    };
    Ok(utc_secs as f64)
}

pub(crate) fn traceback_limit_from_bits(
    _py: &PyToken<'_>,
    limit_bits: u64,
) -> Result<Option<usize>, u64> {
    let obj = obj_from_bits(limit_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(limit) = to_i64(obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "limit must be an integer",
        ));
    };
    if limit < 0 {
        return Ok(Some(0));
    }
    Ok(Some(limit as usize))
}

pub(crate) fn traceback_frames(
    _py: &PyToken<'_>,
    tb_bits: u64,
    limit: Option<usize>,
) -> Vec<(String, i64, String)> {
    if obj_from_bits(tb_bits).is_none() {
        return Vec::new();
    }
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<(String, i64, String)> = Vec::new();
    let mut current_bits = tb_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if let Some(max) = limit
            && out.len() >= max
        {
            break;
        }
        if depth > 512 {
            break;
        }
        let tb_obj = obj_from_bits(current_bits);
        let Some(tb_ptr) = tb_obj.as_ptr() else {
            break;
        };
        let (frame_bits, line, next_bits, had_tb_fields) = unsafe {
            let dict_bits = instance_dict_bits(tb_ptr);
            let mut frame_bits = MoltObject::none().bits();
            let mut line = 0i64;
            let mut next_bits = MoltObject::none().bits();
            let mut had_tb_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_frame_bits) {
                    frame_bits = bits;
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_lineno_bits) {
                    if let Some(val) = to_i64(obj_from_bits(bits)) {
                        line = val;
                    }
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_next_bits) {
                    next_bits = bits;
                    had_tb_fields = true;
                }
            }
            (frame_bits, line, next_bits, had_tb_fields)
        };
        if !had_tb_fields {
            break;
        }
        let (filename, func_name, frame_line) = unsafe {
            let mut filename = "<unknown>".to_string();
            let mut func_name = "<module>".to_string();
            let mut frame_line = line;
            if let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() {
                let dict_bits = instance_dict_bits(frame_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_bits)
                        && let Some(val) = to_i64(obj_from_bits(bits))
                    {
                        frame_line = val;
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_bits)
                        && let Some(code_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(code_ptr) == TYPE_ID_CODE
                    {
                        let filename_bits = code_filename_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                            filename = name;
                        }
                        let name_bits = code_name_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits))
                            && !name.is_empty()
                        {
                            func_name = name;
                        }
                    }
                }
            }
            (filename, func_name, frame_line)
        };
        let final_line = if line > 0 { line } else { frame_line };
        out.push((filename, final_line, func_name));
        current_bits = next_bits;
        depth += 1;
    }
    out
}

pub(crate) fn traceback_source_line_native(
    _py: &PyToken<'_>,
    filename: &str,
    lineno: i64,
) -> String {
    if lineno <= 0 {
        return String::new();
    }
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "traceback.source_line",
        "fs.read",
        AuditArgs::Path(filename.to_string()),
        allowed,
    );
    if !allowed {
        return String::new();
    }
    let Ok(file) = std::fs::File::open(filename) else {
        return String::new();
    };
    let reader = BufReader::new(file);
    let target = lineno as usize;
    for (idx, line_result) in reader.lines().enumerate() {
        if idx + 1 == target {
            if let Ok(line) = line_result {
                return line;
            }
            return String::new();
        }
    }
    String::new()
}

pub(crate) fn traceback_line_trim_bounds(line: &str) -> Option<(i64, i64)> {
    if line.is_empty() {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end <= start {
        return None;
    }
    Some((start as i64, end as i64))
}

pub(crate) fn traceback_infer_column_offsets(line: &str) -> (i64, i64) {
    if line.is_empty() {
        return (0, 0);
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    if start >= chars.len() {
        return (0, 0);
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let trimmed: String = chars[start..end].iter().collect();
    let mut highlighted_start = start;
    if let Some(rest) = trimmed
        .strip_prefix("return ")
        .or_else(|| trimmed.strip_prefix("raise "))
        .or_else(|| trimmed.strip_prefix("yield "))
        .or_else(|| trimmed.strip_prefix("await "))
        .or_else(|| trimmed.strip_prefix("assert "))
    {
        highlighted_start = end.saturating_sub(rest.chars().count());
        while highlighted_start < end && chars[highlighted_start].is_whitespace() {
            highlighted_start += 1;
        }
    } else {
        let trimmed_chars: Vec<char> = trimmed.chars().collect();
        for idx in 0..trimmed_chars.len() {
            if trimmed_chars[idx] != '=' {
                continue;
            }
            let prev = if idx > 0 {
                Some(trimmed_chars[idx - 1])
            } else {
                None
            };
            let next = if idx + 1 < trimmed_chars.len() {
                Some(trimmed_chars[idx + 1])
            } else {
                None
            };
            if matches!(prev, Some('=' | '!' | '<' | '>' | ':')) || matches!(next, Some('=')) {
                continue;
            }
            let mut rhs_start = start + idx + 1;
            while rhs_start < end && chars[rhs_start].is_whitespace() {
                rhs_start += 1;
            }
            if rhs_start < end {
                highlighted_start = rhs_start;
            }
            break;
        }
    }
    let col = highlighted_start as i64;
    let end_col = end.max(highlighted_start) as i64;
    if end_col <= col {
        (col, col + 1)
    } else {
        (col, end_col)
    }
}

pub(crate) fn traceback_format_caret_line_native(
    line: &str,
    mut colno: i64,
    mut end_colno: i64,
) -> String {
    if line.is_empty() || colno < 0 {
        return String::new();
    }
    let text_len = line.chars().count() as i64;
    if text_len <= 0 {
        return String::new();
    }
    if end_colno < colno {
        end_colno = colno;
    }
    if colno > text_len {
        colno = text_len;
    }
    if end_colno > text_len {
        end_colno = text_len;
    }
    let Some((trim_start, trim_end)) = traceback_line_trim_bounds(line) else {
        return String::new();
    };
    if colno < trim_start {
        colno = trim_start;
    }
    if end_colno > trim_end {
        end_colno = trim_end;
    }
    if end_colno <= colno {
        return String::new();
    }
    let width = (end_colno - colno) as usize;
    let col_usize = colno as usize;
    let mut out = String::with_capacity(4 + col_usize + width + 1);
    out.push_str("    ");
    for ch in line.chars().take(col_usize) {
        if ch == '\t' {
            out.push('\t');
        } else {
            out.push(' ');
        }
    }

    // CPython 3.12 uses ^ for the "anchor" (operator, dot, paren) and ~ for
    // the rest.  Find the anchor within the highlighted region by scanning
    // for operator tokens in the source text.
    let chars: Vec<char> = line.chars().skip(col_usize).take(width).collect();
    let anchor = find_caret_anchor(&chars);
    match anchor {
        Some((a_start, a_end)) => {
            for i in 0..width {
                if i >= a_start && i < a_end {
                    out.push('^');
                } else {
                    out.push('~');
                }
            }
        }
        None => {
            for _ in 0..width {
                out.push('^');
            }
        }
    }
    out.push('\n');
    out
}

/// Find the binary-operator anchor position within a highlighted region.
/// Returns (start, end) as char offsets within `region`, or None if the whole
/// region should use `^`.  Matches CPython 3.12 which only uses `~`/`^` for
/// binary operations — attribute access, calls, subscripts all use `^`.
fn find_caret_anchor(region: &[char]) -> Option<(usize, usize)> {
    if region.len() <= 2 {
        return None; // too short for binary op pattern
    }
    // Binary operators: find a run of operator chars in the interior,
    // indicating `operand OP operand`.  Whitespace around the operator
    // is expected (e.g. `1 / 0` has spaces around `/`).
    let op_char = |c: char| {
        matches!(
            c,
            '+' | '-' | '*' | '/' | '%' | '|' | '&' | '^' | '~' | '<' | '>' | '=' | '!' | '@'
        )
    };
    let mut i = 0;
    // Skip leading non-operator chars (left operand + whitespace).
    while i < region.len() && !op_char(region[i]) {
        i += 1;
    }
    if i == 0 || i >= region.len() {
        return None; // no left operand or no operator
    }
    let op_start = i;
    // Consume the operator token (may be multi-char: //, **, <<, etc.)
    while i < region.len() && op_char(region[i]) {
        i += 1;
    }
    let op_end = i;
    // Skip whitespace after operator.
    while i < region.len() && region[i] == ' ' {
        i += 1;
    }
    // Must have a right operand remaining.
    if i >= region.len() {
        return None;
    }
    // Verify left operand has non-whitespace content before operator.
    let left_has_content = region[..op_start].iter().any(|c| !c.is_whitespace());
    if !left_has_content {
        return None;
    }
    Some((op_start, op_end))
}

#[cfg(test)]
mod traceback_format_tests {
    use super::{
        PythonVersionInfo, format_sys_version, traceback_format_caret_line_native,
        traceback_infer_column_offsets,
    };

    #[test]
    fn infer_column_offsets_prefers_rhs_for_assignment() {
        let (col, end_col) = traceback_infer_column_offsets("total = left + right   ");
        assert_eq!(col, 8);
        assert!(end_col > col);
    }

    #[test]
    fn infer_column_offsets_skips_return_keyword() {
        let (col, end_col) = traceback_infer_column_offsets("    return value");
        assert_eq!(col, 11);
        assert_eq!(end_col, 16);
    }

    #[test]
    fn caret_line_preserves_tabs_for_alignment() {
        let line = "\titem = source";
        let caret = traceback_format_caret_line_native(line, 1, 5);
        assert!(caret.starts_with("    \t"));
        assert!(caret.contains("^^^^"));
    }

    #[test]
    fn caret_line_omits_invalid_ranges() {
        let line = "value = source";
        assert!(traceback_format_caret_line_native(line, 0, 0).is_empty());
        assert!(traceback_format_caret_line_native(line, 10, 5).is_empty());
    }

    #[test]
    fn format_sys_version_final_release() {
        let info = PythonVersionInfo {
            major: 3,
            minor: 12,
            micro: 7,
            releaselevel: "final".to_string(),
            serial: 0,
        };
        assert_eq!(format_sys_version(&info), "3.12.7 (molt)");
    }

    #[test]
    fn format_sys_version_candidate_release() {
        let info = PythonVersionInfo {
            major: 3,
            minor: 13,
            micro: 0,
            releaselevel: "candidate".to_string(),
            serial: 2,
        };
        assert_eq!(format_sys_version(&info), "3.13.0rc2 (molt)");
    }
}

pub(crate) fn traceback_format_exception_only_line(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
) -> String {
    let value_obj = obj_from_bits(value_bits);
    if let Some(exc_ptr) = value_obj.as_ptr() {
        unsafe {
            if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                let mut kind = "Exception".to_string();
                let class_bits = exception_class_bits(exc_ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let name_bits = class_name_bits(class_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        kind = name;
                    }
                }
                let message = format_exception_message(_py, exc_ptr);
                if message.is_empty() {
                    return format!("{kind}\n");
                }
                return format!("{kind}: {message}\n");
            }
        }
    }
    let type_name = if !obj_from_bits(exc_type_bits).is_none() {
        if let Some(tp_ptr) = obj_from_bits(exc_type_bits).as_ptr() {
            unsafe {
                if object_type_id(tp_ptr) == TYPE_ID_TYPE {
                    let name_bits = class_name_bits(tp_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        name
                    } else {
                        "Exception".to_string()
                    }
                } else {
                    class_name_for_error(type_of_bits(_py, exc_type_bits))
                }
            }
        } else {
            "Exception".to_string()
        }
    } else if !value_obj.is_none() {
        class_name_for_error(type_of_bits(_py, value_bits))
    } else {
        "Exception".to_string()
    };
    if value_obj.is_none() {
        return format!("{type_name}\n");
    }
    let text = format_obj_str(_py, value_obj);
    if text.is_empty() {
        format!("{type_name}\n")
    } else {
        format!("{type_name}: {text}\n")
    }
}

pub(crate) fn traceback_exception_type_bits(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return exception_class_bits(ptr);
            }
        }
    }
    if obj_from_bits(value_bits).is_none() {
        MoltObject::none().bits()
    } else {
        type_of_bits(_py, value_bits)
    }
}

pub(crate) fn traceback_exception_trace_bits(value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return exception_trace_bits(ptr);
            }
        }
    }
    MoltObject::none().bits()
}

pub(crate) fn traceback_append_exception_single_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    out: &mut Vec<String>,
) {
    if !obj_from_bits(tb_bits).is_none() {
        out.push("Traceback (most recent call last):\n".to_string());
        let payload = traceback_payload_from_source(_py, tb_bits, limit);
        out.extend(traceback_payload_to_formatted_lines(_py, &payload));
    }
    out.push(traceback_format_exception_only_line(
        _py,
        exc_type_bits,
        value_bits,
    ));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn traceback_append_exception_chain_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    chain: bool,
    seen: &mut HashSet<u64>,
    out: &mut Vec<String>,
) {
    if obj_from_bits(value_bits).is_none() || !chain {
        traceback_append_exception_single_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            out,
        );
        return;
    }
    if seen.contains(&value_bits) {
        traceback_append_exception_single_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            out,
        );
        return;
    }
    seen.insert(value_bits);
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                let cause_bits = exception_cause_bits(ptr);
                if !obj_from_bits(cause_bits).is_none() {
                    let cause_type_bits = traceback_exception_type_bits(_py, cause_bits);
                    let cause_tb_bits = traceback_exception_trace_bits(cause_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        cause_type_bits,
                        cause_bits,
                        cause_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "The above exception was the direct cause of the following exception:\n\n"
                            .to_string(),
                    );
                    traceback_append_exception_single_lines(
                        _py,
                        exc_type_bits,
                        value_bits,
                        tb_bits,
                        limit,
                        out,
                    );
                    return;
                }
                let context_bits = exception_context_bits(ptr);
                let suppress_context = is_truthy(_py, obj_from_bits(exception_suppress_bits(ptr)));
                if !suppress_context && !obj_from_bits(context_bits).is_none() {
                    let context_type_bits = traceback_exception_type_bits(_py, context_bits);
                    let context_tb_bits = traceback_exception_trace_bits(context_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        context_type_bits,
                        context_bits,
                        context_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "During handling of the above exception, another exception occurred:\n\n"
                            .to_string(),
                    );
                    traceback_append_exception_single_lines(
                        _py,
                        exc_type_bits,
                        value_bits,
                        tb_bits,
                        limit,
                        out,
                    );
                    return;
                }
            }
        }
    }
    traceback_append_exception_single_lines(_py, exc_type_bits, value_bits, tb_bits, limit, out);
}

pub(crate) fn traceback_lines_to_list(_py: &PyToken<'_>, lines: &[String]) -> u64 {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(lines.len());
    for line in lines {
        let ptr = alloc_string(_py, line.as_bytes());
        if ptr.is_null() {
            for bits in bits_vec {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        bits_vec.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, bits_vec.as_slice());
    for bits in bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[derive(Clone)]
pub(crate) struct TracebackPayloadFrame {
    pub(crate) filename: String,
    pub(crate) lineno: i64,
    pub(crate) end_lineno: i64,
    pub(crate) colno: i64,
    pub(crate) end_colno: i64,
    pub(crate) name: String,
    pub(crate) line: String,
}

#[derive(Clone)]
pub(crate) struct TracebackExceptionChainNode {
    pub(crate) value_bits: u64,
    pub(crate) frames: Vec<TracebackPayloadFrame>,
    pub(crate) suppress_context: bool,
    pub(crate) cause_index: Option<usize>,
    pub(crate) context_index: Option<usize>,
}

pub(crate) fn traceback_split_molt_symbol(name: &str) -> (String, String) {
    if let Some((module_hint, func)) = name.split_once("__")
        && !module_hint.is_empty()
    {
        let func_name = if func.is_empty() { name } else { func };
        return (format!("<molt:{module_hint}>"), func_name.to_string());
    }
    ("<molt>".to_string(), name.to_string())
}

pub(crate) fn traceback_payload_from_traceback(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for (filename, lineno, name) in traceback_frames(_py, source_bits, limit) {
        let line = traceback_source_line_native(_py, &filename, lineno);
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
            name,
            line,
        });
    }
    out
}

pub(crate) fn traceback_payload_from_frame_chain(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    static F_BACK_NAME: AtomicU64 = AtomicU64::new(0);
    let f_back_name = intern_static_name(_py, &F_BACK_NAME, b"f_back");
    let f_code_name = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_name =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    let mut current_bits = source_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 1024 {
            break;
        }
        let Some(frame_ptr) = obj_from_bits(current_bits).as_ptr() else {
            break;
        };
        let (code_bits, lineno, back_bits, had_frame_fields) = unsafe {
            let dict_bits = instance_dict_bits(frame_ptr);
            let mut code_bits = MoltObject::none().bits();
            let mut lineno = 0i64;
            let mut back_bits = MoltObject::none().bits();
            let mut had_frame_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_name) {
                    code_bits = bits;
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_name) {
                    if let Some(value) = to_i64(obj_from_bits(bits)) {
                        lineno = value;
                    }
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_back_name) {
                    back_bits = bits;
                    had_frame_fields = true;
                }
            }
            (code_bits, lineno, back_bits, had_frame_fields)
        };
        if !had_frame_fields {
            break;
        }

        let mut filename = "<unknown>".to_string();
        let mut name = "<module>".to_string();
        if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(code_ptr) == TYPE_ID_CODE {
                    let filename_bits = code_filename_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                        filename = value;
                    }
                    let name_bits = code_name_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(name_bits))
                        && !value.is_empty()
                    {
                        name = value;
                    }
                }
            }
        }
        let line = traceback_source_line_native(_py, &filename, lineno);
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
            name,
            line,
        });
        current_bits = back_bits;
        depth += 1;
    }
    out.reverse();
    if let Some(max) = limit
        && out.len() > max
    {
        return out[out.len() - max..].to_vec();
    }
    out
}

pub(crate) fn traceback_payload_from_entry(
    _py: &PyToken<'_>,
    entry_bits: u64,
) -> Option<TracebackPayloadFrame> {
    if obj_from_bits(entry_bits).is_none() {
        return None;
    }
    let entry_obj = obj_from_bits(entry_bits);
    if let Some(entry_ptr) = entry_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(entry_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(entry_ptr);
                if elems.is_empty() {
                    return None;
                }
                if elems.len() == 1 {
                    return traceback_payload_from_entry(_py, elems[0]);
                }
                if elems.len() >= 7 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let end_lineno = to_i64(obj_from_bits(elems[2])).unwrap_or(lineno);
                    let mut colno = to_i64(obj_from_bits(elems[3])).unwrap_or(0);
                    let mut end_colno = to_i64(obj_from_bits(elems[4])).unwrap_or(colno.max(0));
                    let name = format_obj_str(_py, obj_from_bits(elems[5]));
                    let line = if obj_from_bits(elems[6]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[6]))
                    };
                    if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                        let inferred = traceback_infer_column_offsets(&line);
                        colno = inferred.0;
                        end_colno = inferred.1;
                    }
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 4 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = if obj_from_bits(elems[3]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[3]))
                    };
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 3 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = traceback_source_line_native(_py, &filename, lineno);
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() == 2 {
                    let first_obj = obj_from_bits(elems[0]);
                    let second_obj = obj_from_bits(elems[1]);
                    if let (Some(filename), Some(lineno)) =
                        (string_obj_to_owned(first_obj), to_i64(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(lineno), Some(filename)) =
                        (to_i64(first_obj), string_obj_to_owned(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(symbol), Some(_name)) = (
                        string_obj_to_owned(first_obj),
                        string_obj_to_owned(second_obj),
                    ) {
                        let (filename, name) = traceback_split_molt_symbol(&symbol);
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno: 0,
                            end_lineno: 0,
                            colno: 0,
                            end_colno: 0,
                            name,
                            line: String::new(),
                        });
                    }
                }
                return None;
            }
            if type_id == TYPE_ID_DICT {
                static FILENAME_NAME: AtomicU64 = AtomicU64::new(0);
                static LINENO_NAME: AtomicU64 = AtomicU64::new(0);
                static NAME_NAME: AtomicU64 = AtomicU64::new(0);
                static LINE_NAME: AtomicU64 = AtomicU64::new(0);
                static END_LINENO_NAME: AtomicU64 = AtomicU64::new(0);
                static COLNO_NAME: AtomicU64 = AtomicU64::new(0);
                static END_COLNO_NAME: AtomicU64 = AtomicU64::new(0);
                let filename_key = intern_static_name(_py, &FILENAME_NAME, b"filename");
                let lineno_key = intern_static_name(_py, &LINENO_NAME, b"lineno");
                let name_key = intern_static_name(_py, &NAME_NAME, b"name");
                let line_key = intern_static_name(_py, &LINE_NAME, b"line");
                let end_lineno_key = intern_static_name(_py, &END_LINENO_NAME, b"end_lineno");
                let colno_key = intern_static_name(_py, &COLNO_NAME, b"colno");
                let end_colno_key = intern_static_name(_py, &END_COLNO_NAME, b"end_colno");
                let filename_bits = dict_get_in_place(_py, entry_ptr, filename_key)?;
                let lineno_bits = dict_get_in_place(_py, entry_ptr, lineno_key)?;
                let filename = format_obj_str(_py, obj_from_bits(filename_bits));
                let lineno = to_i64(obj_from_bits(lineno_bits)).unwrap_or(0);
                let name = dict_get_in_place(_py, entry_ptr, name_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| "<module>".to_string());
                let line = dict_get_in_place(_py, entry_ptr, line_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| traceback_source_line_native(_py, &filename, lineno));
                let (mut colno, mut end_colno) = traceback_infer_column_offsets(&line);
                if let Some(value) = dict_get_in_place(_py, entry_ptr, colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    colno = value;
                }
                if let Some(value) = dict_get_in_place(_py, entry_ptr, end_colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    end_colno = value;
                }
                if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                    let inferred = traceback_infer_column_offsets(&line);
                    colno = inferred.0;
                    end_colno = inferred.1;
                }
                let end_lineno = dict_get_in_place(_py, entry_ptr, end_lineno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(lineno);
                return Some(TracebackPayloadFrame {
                    filename,
                    lineno,
                    end_lineno,
                    colno,
                    end_colno,
                    name,
                    line,
                });
            }
        }
    }

    if let Some(value) = string_obj_to_owned(entry_obj) {
        let (filename, name) = traceback_split_molt_symbol(&value);
        return Some(TracebackPayloadFrame {
            filename,
            lineno: 0,
            end_lineno: 0,
            colno: 0,
            end_colno: 0,
            name,
            line: String::new(),
        });
    }

    let mut from_tb = traceback_payload_from_traceback(_py, entry_bits, Some(1));
    if let Some(frame) = from_tb.pop() {
        return Some(frame);
    }
    let mut from_frame = traceback_payload_from_frame_chain(_py, entry_bits, Some(1));
    from_frame.pop()
}

pub(crate) fn traceback_payload_from_entries(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
        return Vec::new();
    };
    let type_id = unsafe { object_type_id(source_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Vec::new();
    }
    let elems: Vec<u64> = unsafe { seq_vec_ref(source_ptr).to_vec() };
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for bits in elems {
        if let Some(frame) = traceback_payload_from_entry(_py, bits) {
            out.push(frame);
            if let Some(max) = limit
                && out.len() >= max
            {
                break;
            }
        }
    }
    out
}

pub(crate) fn traceback_payload_from_source(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    let from_entries = traceback_payload_from_entries(_py, source_bits, limit);
    if !from_entries.is_empty() {
        return from_entries;
    }
    let from_tb = traceback_payload_from_traceback(_py, source_bits, limit);
    if !from_tb.is_empty() {
        return from_tb;
    }
    let from_frame = traceback_payload_from_frame_chain(_py, source_bits, limit);
    if !from_frame.is_empty() {
        return from_frame;
    }
    if let Some(frame) = traceback_payload_from_entry(_py, source_bits) {
        return vec![frame];
    }
    Vec::new()
}

pub(crate) fn traceback_payload_to_list(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> u64 {
    let mut tuples: Vec<u64> = Vec::new();
    for frame in payload {
        let filename_ptr = alloc_string(_py, frame.filename.as_bytes());
        if filename_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, frame.name.as_bytes());
        if name_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let line_ptr = alloc_string(_py, frame.line.as_bytes());
        if line_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let lineno_bits = MoltObject::from_int(frame.lineno).bits();
        let end_lineno_bits = MoltObject::from_int(frame.end_lineno).bits();
        let colno_bits = MoltObject::from_int(frame.colno).bits();
        let end_colno_bits = MoltObject::from_int(frame.end_colno).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let line_bits = MoltObject::from_ptr(line_ptr).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                filename_bits,
                lineno_bits,
                end_lineno_bits,
                colno_bits,
                end_colno_bits,
                name_bits,
                line_bits,
            ],
        );
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, end_lineno_bits);
        dec_ref_bits(_py, colno_bits);
        dec_ref_bits(_py, end_colno_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, line_bits);
        if tuple_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        tuples.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, tuples.as_slice());
    for bits in tuples {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

pub(crate) fn traceback_payload_frame_source_lines(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut first_line = frame.line.clone();
    let mut first_colno = frame.colno;
    let mut first_end_colno = frame.end_colno;
    if first_line.is_empty() {
        first_line = traceback_source_line_native(_py, &frame.filename, frame.lineno);
        if first_line.is_empty() {
            return lines;
        }
        if first_colno < 0 || first_end_colno <= first_colno {
            let (col, end_col) = traceback_infer_column_offsets(&first_line);
            first_colno = col;
            first_end_colno = end_col;
        }
    }

    let span_end = frame.end_lineno.max(frame.lineno);
    if span_end <= frame.lineno || frame.lineno <= 0 || (span_end - frame.lineno) > 64 {
        lines.push(format!("    {}\n", first_line));
        let caret = traceback_format_caret_line_native(&first_line, first_colno, first_end_colno);
        if !caret.is_empty() {
            lines.push(caret);
        }
        return lines;
    }

    for lineno in frame.lineno..=span_end {
        let text = if lineno == frame.lineno {
            first_line.clone()
        } else {
            traceback_source_line_native(_py, &frame.filename, lineno)
        };
        if text.is_empty() {
            continue;
        }
        lines.push(format!("    {}\n", text));

        let text_len = text.chars().count() as i64;
        if text_len <= 0 {
            continue;
        }
        let (trim_start, trim_end) = traceback_line_trim_bounds(&text).unwrap_or((0, text_len));
        let (start, end) = if lineno == frame.lineno {
            let start = if first_colno >= 0 {
                first_colno
            } else {
                trim_start
            };
            let end = if lineno == span_end {
                if first_end_colno > start {
                    first_end_colno
                } else {
                    trim_end
                }
            } else {
                trim_end
            };
            (start, end)
        } else if lineno == span_end {
            let end = if frame.end_colno > trim_start {
                frame.end_colno
            } else {
                trim_end
            };
            (trim_start, end)
        } else {
            (trim_start, trim_end)
        };
        let caret = traceback_format_caret_line_native(&text, start, end);
        if !caret.is_empty() {
            lines.push(caret);
        }
    }

    lines
}

pub(crate) fn traceback_payload_to_formatted_lines(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for frame in payload {
        lines.push(format!(
            "  File \"{}\", line {}, in {}\n",
            frame.filename, frame.lineno, frame.name
        ));
        lines.extend(traceback_payload_frame_source_lines(_py, frame));
    }
    lines
}

pub(crate) fn traceback_exception_components_payload(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let payload = traceback_payload_from_source(_py, tb_bits, limit);
    let frames_bits = traceback_payload_to_list(_py, &payload);
    if obj_from_bits(frames_bits).is_none() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    if !obj_from_bits(cause_bits).is_none() {
        inc_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        inc_ref_bits(_py, context_bits);
    }
    let suppress_bits = MoltObject::from_bool(suppress_context).bits();
    let tuple_ptr = alloc_tuple(_py, &[frames_bits, cause_bits, context_bits, suppress_bits]);
    dec_ref_bits(_py, frames_bits);
    if !obj_from_bits(cause_bits).is_none() {
        dec_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        dec_ref_bits(_py, context_bits);
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

pub(crate) fn traceback_exception_chain_collect(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
    nodes: &mut Vec<TracebackExceptionChainNode>,
    seen: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize, u64> {
    if depth > 1024 {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "traceback exception chain recursion too deep",
        ));
    }
    if let Some(index) = seen.get(&value_bits) {
        return Ok(*index);
    }
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let frames = traceback_payload_from_source(_py, tb_bits, limit);
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    let index = nodes.len();
    seen.insert(value_bits, index);
    nodes.push(TracebackExceptionChainNode {
        value_bits,
        frames,
        suppress_context,
        cause_index: None,
        context_index: None,
    });

    if !obj_from_bits(cause_bits).is_none() {
        let Some(cause_ptr) = obj_from_bits(cause_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __cause__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __cause__ must be an exception instance or None",
                ));
            }
        }
        let cause_index =
            traceback_exception_chain_collect(_py, cause_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].cause_index = Some(cause_index);
    }

    if !suppress_context && !obj_from_bits(context_bits).is_none() {
        let Some(context_ptr) = obj_from_bits(context_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __context__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(context_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __context__ must be an exception instance or None",
                ));
            }
        }
        let context_index =
            traceback_exception_chain_collect(_py, context_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].context_index = Some(context_index);
    }

    Ok(index)
}

pub(crate) fn traceback_exception_chain_payload_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let mut nodes: Vec<TracebackExceptionChainNode> = Vec::new();
    let mut seen: HashMap<u64, usize> = HashMap::new();
    traceback_exception_chain_collect(_py, value_bits, limit, &mut nodes, &mut seen, 0)?;

    let mut tuple_bits: Vec<u64> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let frames_bits = traceback_payload_to_list(_py, &node.frames);
        if obj_from_bits(frames_bits).is_none() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        inc_ref_bits(_py, node.value_bits);
        let suppress_bits = MoltObject::from_bool(node.suppress_context).bits();
        let cause_bits = match node.cause_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let context_bits = match node.context_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                node.value_bits,
                frames_bits,
                suppress_bits,
                cause_bits,
                context_bits,
            ],
        );
        dec_ref_bits(_py, node.value_bits);
        dec_ref_bits(_py, frames_bits);
        if node.cause_index.is_some() {
            dec_ref_bits(_py, cause_bits);
        }
        if node.context_index.is_some() {
            dec_ref_bits(_py, context_bits);
        }
        if tuple_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }

    let list_ptr = alloc_list(_py, tuple_bits.as_slice());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

// ---------------------------------------------------------------------------
// Runtime initialization from manifest environment variables
// ---------------------------------------------------------------------------

/// Initialize the resource tracker from environment variables set by the
/// capability manifest. Called during runtime startup.
///
/// Reads: MOLT_RESOURCE_MAX_MEMORY, MOLT_RESOURCE_MAX_DURATION_MS,
///        MOLT_RESOURCE_MAX_ALLOCATIONS, MOLT_RESOURCE_MAX_RECURSION_DEPTH
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_resources() {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    use std::time::Duration;

    let max_memory = std::env::var("MOLT_RESOURCE_MAX_MEMORY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let max_duration_ms = std::env::var("MOLT_RESOURCE_MAX_DURATION_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    let max_allocations = std::env::var("MOLT_RESOURCE_MAX_ALLOCATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let max_recursion_depth = std::env::var("MOLT_RESOURCE_MAX_RECURSION_DEPTH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let has_any = max_memory.is_some()
        || max_duration_ms.is_some()
        || max_allocations.is_some()
        || max_recursion_depth.is_some();

    if has_any {
        let limits = ResourceLimits {
            max_memory,
            max_duration: max_duration_ms.map(Duration::from_millis),
            max_allocations,
            max_recursion_depth,
            max_operation_result_bytes: None,
        };
        set_tracker(Box::new(LimitedTracker::new(&limits)));
    }
}

/// Initialize the audit sink from environment variables.
///
/// Reads: MOLT_AUDIT_ENABLED, MOLT_AUDIT_SINK
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_audit() {
    use crate::audit::{JsonLinesSink, NullSink, StderrSink, set_audit_sink};

    let enabled = std::env::var("MOLT_AUDIT_ENABLED")
        .ok()
        .map(|s| s == "1")
        .unwrap_or(false);

    if !enabled {
        return;
    }

    let sink_type = std::env::var("MOLT_AUDIT_SINK").unwrap_or_else(|_| "stderr".into());
    match sink_type.as_str() {
        "jsonl" => {
            set_audit_sink(Box::new(JsonLinesSink::new(std::io::stderr())));
        }
        "stderr" => {
            set_audit_sink(Box::new(StderrSink));
        }
        _ => {
            set_audit_sink(Box::new(NullSink));
        }
    }
}

/// Initialize IO mode from environment variable.
///
/// Reads: MOLT_IO_MODE (real | virtual | callback)
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_io_mode() {
    use crate::vfs::caps::{IoMode, set_io_mode};

    let mode_str = std::env::var("MOLT_IO_MODE").unwrap_or_else(|_| "real".into());
    let mode = match mode_str.as_str() {
        "virtual" => IoMode::Virtual,
        "callback" => IoMode::Callback,
        _ => IoMode::Real,
    };
    set_io_mode(mode);
}

/// Initialize the type gate from environment variable.
///
/// When MOLT_TYPE_GATE=1, the compiler rejects untyped code in
/// capability-touching paths. Currently a no-op stub — the actual
/// type checking is performed at compile time in the frontend.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_type_gate() {
    let enabled = std::env::var("MOLT_TYPE_GATE")
        .ok()
        .map(|s| s == "1")
        .unwrap_or(false);
    if enabled {
        // Type gate is enforced at compile time. This runtime stub exists
        // for forward compatibility — a future version may perform runtime
        // type narrowing checks here.
        TYPE_GATE_ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

static TYPE_GATE_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
