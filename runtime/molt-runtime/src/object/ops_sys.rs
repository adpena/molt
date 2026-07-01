// System, GC, time, signal, traceback, profiling, and related runtime support.
// Split from ops.rs for compilation-unit size reduction.

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::object::ops::{range_components_bigint, range_len_bigint};
use crate::object::ops_string::{
    push_wtf8_codepoint, utf8_codepoint_count_cached, wtf8_codepoint_at,
};
use crate::state::runtime_state::PythonVersionInfo;
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_traits::{Signed, ToPrimitive, Zero};
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::CStr;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::CString;
use std::io::{BufRead, BufReader};
use std::sync::{Mutex, OnceLock};

#[path = "ops_sys_time.rs"]
mod ops_sys_time;
pub(crate) use ops_sys_time::*;
#[path = "ops_sys_traceback.rs"]
mod ops_sys_traceback;
pub(crate) use ops_sys_traceback::*;
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

/// C-ABI view of [`runtime_target_at_least`]. Exposed so satellite stdlib
/// modules (which link `molt-runtime-core` via the FFI bridge and therefore have
/// no direct access to the host `RuntimeState`) can version-gate behavior
/// IDENTICALLY to their in-tree twin — e.g. csv's `QUOTE_STRINGS`/`QUOTE_NOTNULL`
/// reader semantics, which only apply on CPython 3.13+. Returns `1` when the
/// configured target Python is `>= major.minor`, else `0`.
///
/// # Safety
/// Pure read of the per-runtime target-version cache behind the GIL; no pointer
/// arguments. The `unsafe` marker exists only because the C ABI demands it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_target_at_least(major: i64, minor: i64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        i64::from(runtime_target_at_least(_py, major, minor))
    })
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

// molt_set_argv, molt_set_argv_utf16 live in ops.rs

// ---------------------------------------------------------------------------

fn parse_positive_integer_resource_env<T>(name: &'static str) -> Result<Option<T>, String>
where
    T: Default + PartialEq + std::str::FromStr,
{
    let raw = match std::env::var(name) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} must be valid UTF-8"));
        }
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "{name} must be a positive integer, got an empty value"
        ));
    }
    let parsed = trimmed
        .parse::<T>()
        .map_err(|_| format!("{name} must be a positive integer, got {raw:?}"))?;
    if parsed == T::default() {
        return Err(format!("{name} must be a positive integer, got 0"));
    }
    Ok(Some(parsed))
}

/// Resolve the memory limit from the two coherent env sources.
///
/// `MOLT_MEMORY_LIMIT` is the ergonomic, human-readable front door (`"512M"`,
/// `"2G"`); `MOLT_RESOURCE_MAX_MEMORY` is the canonical raw-byte field emitted
/// by the capability manifest. Both resolve to the SAME
/// `ResourceLimits.max_memory` field — there is exactly one enforcement path.
/// When both are set the user-facing alias wins and a one-line override notice
/// is printed. A misconfigured value fails loudly (never silently ignored).
fn resolve_memory_limit_from_env() -> Result<Option<usize>, String> {
    let alias = match std::env::var("MOLT_MEMORY_LIMIT") {
        Ok(raw) => Some(
            crate::resource::parse_human_size(&raw)
                .map_err(|e| format!("MOLT_MEMORY_LIMIT: {e}"))?,
        ),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("MOLT_MEMORY_LIMIT must be valid UTF-8".to_string());
        }
    };
    let canonical = parse_positive_integer_resource_env::<usize>("MOLT_RESOURCE_MAX_MEMORY")?;

    match (alias, canonical) {
        (Some(alias_bytes), Some(canonical_bytes)) => {
            if alias_bytes != canonical_bytes {
                eprintln!(
                    "molt: MOLT_MEMORY_LIMIT ({alias_bytes} bytes) overrides \
                     MOLT_RESOURCE_MAX_MEMORY ({canonical_bytes} bytes)"
                );
            }
            Ok(Some(alias_bytes))
        }
        (Some(alias_bytes), None) => Ok(Some(alias_bytes)),
        (None, Some(canonical_bytes)) => Ok(Some(canonical_bytes)),
        (None, None) => Ok(None),
    }
}

fn resource_limits_from_env() -> Result<Option<crate::resource::ResourceLimits>, String> {
    use std::time::Duration;

    let max_memory = resolve_memory_limit_from_env()?;
    let max_duration_ms =
        parse_positive_integer_resource_env::<u64>("MOLT_RESOURCE_MAX_DURATION_MS")?;
    let max_allocations = parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_ALLOCATIONS")?;
    let max_recursion_depth =
        parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_RECURSION_DEPTH")?;

    // Per-operation result caps. These mirror the Python ResourceLimits
    // dataclass fields (max_pow_result / max_repeat_result / max_shift_result /
    // max_string_result) so manifest-declared per-op limits reach the Rust
    // tracker without being silently dropped at the env boundary.
    let max_operation_result_bytes =
        parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_OPERATION_RESULT")?;
    let max_pow_result_bytes = parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_POW_RESULT")?;
    let max_repeat_result_bytes =
        parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_REPEAT_RESULT")?;
    let max_shift_result_bytes =
        parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_SHIFT_RESULT")?;
    let max_string_result_bytes =
        parse_positive_integer_resource_env("MOLT_RESOURCE_MAX_STRING_RESULT")?;

    let has_any = max_memory.is_some()
        || max_duration_ms.is_some()
        || max_allocations.is_some()
        || max_recursion_depth.is_some()
        || max_operation_result_bytes.is_some()
        || max_pow_result_bytes.is_some()
        || max_repeat_result_bytes.is_some()
        || max_shift_result_bytes.is_some()
        || max_string_result_bytes.is_some();
    if !has_any {
        return Ok(None);
    }

    Ok(Some(crate::resource::ResourceLimits {
        max_memory,
        max_duration: max_duration_ms.map(Duration::from_millis),
        max_allocations,
        max_recursion_depth,
        max_operation_result_bytes,
        max_pow_result_bytes,
        max_repeat_result_bytes,
        max_shift_result_bytes,
        max_string_result_bytes,
    }))
}

/// Initialize the resource tracker from environment variables set by the
/// capability manifest. Called during runtime startup.
///
/// Reads (raw-byte canonical fields, all positive integers):
///   MOLT_MEMORY_LIMIT (human-size alias for the memory cap),
///   MOLT_RESOURCE_MAX_MEMORY, MOLT_RESOURCE_MAX_DURATION_MS,
///   MOLT_RESOURCE_MAX_ALLOCATIONS, MOLT_RESOURCE_MAX_RECURSION_DEPTH,
///   MOLT_RESOURCE_MAX_OPERATION_RESULT and the per-op caps
///   MOLT_RESOURCE_MAX_{POW,REPEAT,SHIFT,STRING}_RESULT.
///
/// Two-layer enforcement: the parsed limits install the precise in-VM
/// [`LimitedTracker`] (Layer 1, cross-target, deterministic) via the global
/// factory, and — when a memory cap is set — an OS-level `RLIMIT_AS` backstop
/// (Layer 2, native only) bounds anything that bypasses the tracker. The
/// backstop never replaces the tracker; it only converts a runaway into a clean
/// failure instead of an OOM-kill of the host.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_resources() {
    use crate::resource::{install_address_space_backstop, install_global_limited_tracker};

    match resource_limits_from_env() {
        Ok(Some(limits)) => {
            // Layer 2 (OS backstop) FIRST so the address-space ceiling is in
            // place before any tracker-allocated structures grow. Layer 1
            // remains the deterministic contract.
            if let Some(max_memory) = limits.max_memory {
                install_address_space_backstop(max_memory);
            }
            install_global_limited_tracker(limits);
        }
        Ok(None) => {}
        Err(message) => {
            eprintln!("molt runtime resource configuration error: {message}");
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod runtime_resource_env_tests {
    use super::resource_limits_from_env;
    use std::time::Duration;

    const RESOURCE_ENV_KEYS: &[&str] = &[
        "MOLT_MEMORY_LIMIT",
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
        "MOLT_RESOURCE_MAX_OPERATION_RESULT",
        "MOLT_RESOURCE_MAX_POW_RESULT",
        "MOLT_RESOURCE_MAX_REPEAT_RESULT",
        "MOLT_RESOURCE_MAX_SHIFT_RESULT",
        "MOLT_RESOURCE_MAX_STRING_RESULT",
    ];

    fn clear_resource_env() {
        for key in RESOURCE_ENV_KEYS {
            unsafe { std::env::remove_var(key) };
        }
    }

    #[test]
    fn resource_limits_from_env_rejects_invalid_values() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe { std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "not-a-number") };

        let err = resource_limits_from_env().unwrap_err();

        clear_resource_env();
        assert!(err.contains("MOLT_RESOURCE_MAX_MEMORY"));
        assert!(err.contains("positive integer"));
    }

    #[test]
    fn resource_limits_from_env_rejects_zero_values() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe { std::env::set_var("MOLT_RESOURCE_MAX_DURATION_MS", "0") };

        let err = resource_limits_from_env().unwrap_err();

        clear_resource_env();
        assert!(err.contains("MOLT_RESOURCE_MAX_DURATION_MS"));
        assert!(err.contains("positive integer"));
    }

    #[test]
    fn resource_limits_from_env_builds_positive_limit_set() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe {
            std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "1048576");
            std::env::set_var("MOLT_RESOURCE_MAX_DURATION_MS", "2500");
            std::env::set_var("MOLT_RESOURCE_MAX_ALLOCATIONS", "1000");
            std::env::set_var("MOLT_RESOURCE_MAX_RECURSION_DEPTH", "50");
        }

        let limits = resource_limits_from_env().unwrap().unwrap();

        clear_resource_env();
        assert_eq!(limits.max_memory, Some(1_048_576));
        assert_eq!(limits.max_duration, Some(Duration::from_millis(2500)));
        assert_eq!(limits.max_allocations, Some(1000));
        assert_eq!(limits.max_recursion_depth, Some(50));
    }

    #[test]
    fn resource_limits_from_env_carries_per_operation_caps() {
        // Parity guard: per-op env vars (mirroring the Python ResourceLimits
        // per-op fields) must reach the Rust ResourceLimits without loss.
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe {
            std::env::set_var("MOLT_RESOURCE_MAX_OPERATION_RESULT", "9000");
            std::env::set_var("MOLT_RESOURCE_MAX_POW_RESULT", "1048576");
            std::env::set_var("MOLT_RESOURCE_MAX_REPEAT_RESULT", "2097152");
            std::env::set_var("MOLT_RESOURCE_MAX_SHIFT_RESULT", "3145728");
            std::env::set_var("MOLT_RESOURCE_MAX_STRING_RESULT", "4194304");
        }

        let limits = resource_limits_from_env().unwrap().unwrap();

        clear_resource_env();
        assert_eq!(limits.max_operation_result_bytes, Some(9000));
        assert_eq!(limits.max_pow_result_bytes, Some(1_048_576));
        assert_eq!(limits.max_repeat_result_bytes, Some(2_097_152));
        assert_eq!(limits.max_shift_result_bytes, Some(3_145_728));
        assert_eq!(limits.max_string_result_bytes, Some(4_194_304));
    }

    #[test]
    fn molt_memory_limit_alias_parses_human_size() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe { std::env::set_var("MOLT_MEMORY_LIMIT", "64M") };

        let limits = resource_limits_from_env().unwrap().unwrap();

        clear_resource_env();
        assert_eq!(limits.max_memory, Some(64 * 1024 * 1024));
    }

    #[test]
    fn molt_memory_limit_alias_overrides_canonical_field() {
        // When both are set, the user-facing alias wins (single source of
        // truth; the alias resolves into the SAME max_memory field).
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe {
            std::env::set_var("MOLT_MEMORY_LIMIT", "128M");
            std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "1048576");
        }

        let limits = resource_limits_from_env().unwrap().unwrap();

        clear_resource_env();
        assert_eq!(limits.max_memory, Some(128 * 1024 * 1024));
    }

    #[test]
    fn molt_memory_limit_alias_rejects_garbage() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        unsafe { std::env::set_var("MOLT_MEMORY_LIMIT", "not-a-size") };

        let err = resource_limits_from_env().unwrap_err();

        clear_resource_env();
        assert!(err.contains("MOLT_MEMORY_LIMIT"));
    }

    #[test]
    fn no_resource_env_yields_none() {
        // Without any env set, behavior is unchanged: no limits installed.
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_resource_env();
        let limits = resource_limits_from_env().unwrap();
        assert!(limits.is_none());
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

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn args_sizes_get(argc: *mut u32, argv_buf_size: *mut u32) -> u16;
    fn args_get(argv: *mut *mut u8, argv_buf: *mut u8) -> u16;
}

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
fn collect_wasi_argv_bytes() -> Option<Vec<Vec<u8>>> {
    unsafe {
        let mut argc = 0u32;
        let mut argv_buf_size = 0u32;
        if args_sizes_get(&mut argc, &mut argv_buf_size) != 0 {
            return None;
        }
        let argc_usize = argc as usize;
        let mut argv_ptrs = vec![std::ptr::null_mut(); argc_usize];
        let mut argv_buf = vec![0u8; argv_buf_size as usize];
        let argv_buf_ptr = if argv_buf.is_empty() {
            std::ptr::null_mut()
        } else {
            argv_buf.as_mut_ptr()
        };
        if args_get(argv_ptrs.as_mut_ptr(), argv_buf_ptr) != 0 {
            return None;
        }
        let base = argv_buf.as_ptr() as usize;
        let end = base.saturating_add(argv_buf.len());
        let mut out = Vec::with_capacity(argc_usize);
        for ptr in argv_ptrs {
            if ptr.is_null() {
                return None;
            }
            let addr = ptr as usize;
            if addr < base || addr >= end {
                return None;
            }
            let cstr = CStr::from_ptr(ptr.cast::<i8>());
            out.push(cstr.to_bytes().to_vec());
        }
        Some(out)
    }
}

#[cfg(not(all(target_arch = "wasm32", not(feature = "wasm_freestanding"))))]
fn collect_wasi_argv_bytes() -> Option<Vec<Vec<u8>>> {
    None
}

#[inline]
fn trace_len_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_LEN").as_deref() == Ok("1"))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_len(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        if trace_len_enabled() {
            eprintln!(
                "molt_len arg_type={} bits=0x{:x} pending={}",
                type_name(_py, obj),
                val,
                exception_pending(_py),
            );
        }
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    return MoltObject::from_int(count).bits();
                }
                if type_id == TYPE_ID_BYTES {
                    return MoltObject::from_int(bytes_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    return MoltObject::from_int(bytes_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
                    if memoryview_ndim(ptr) == 0 {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "0-dim memory has no length",
                        );
                    }
                    return MoltObject::from_int(memoryview_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_LIST
                    || type_id == TYPE_ID_LIST_INT
                    || type_id == TYPE_ID_LIST_BOOL
                {
                    return MoltObject::from_int(list_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_TUPLE {
                    return MoltObject::from_int(tuple_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_INTARRAY {
                    return MoltObject::from_int(intarray_len(ptr) as i64).bits();
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(dict_len(dict_ptr) as i64).bits();
                }
                if type_id == TYPE_ID_SET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_FROZENSET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    return MoltObject::from_int(dict_view_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_RANGE {
                    let Some((start, stop, step)) = range_components_bigint(ptr) else {
                        return MoltObject::none().bits();
                    };
                    let len = range_len_bigint(&start, &stop, &step);
                    return int_bits_from_bigint(_py, len);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__len__") {
                    let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                    dec_ref_bits(_py, name_bits);
                    if let Some(call_bits) = call_bits {
                        exception_stack_push();
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            if i < 0 {
                                if res_obj.as_ptr().is_some() {
                                    dec_ref_bits(_py, res_bits);
                                }
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                            }
                            if res_obj.as_ptr().is_some() {
                                dec_ref_bits(_py, res_bits);
                            }
                            return MoltObject::from_int(i).bits();
                        }
                        if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                            let big = bigint_ref(big_ptr);
                            if big.is_negative() {
                                dec_ref_bits(_py, res_bits);
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                            }
                            let Some(len) = big.to_usize() else {
                                dec_ref_bits(_py, res_bits);
                                return raise_exception::<_>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                            };
                            if len > i64::MAX as usize {
                                dec_ref_bits(_py, res_bits);
                                return raise_exception::<_>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                            }
                            dec_ref_bits(_py, res_bits);
                            return MoltObject::from_int(len as i64).bits();
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        let msg =
                            format!("'{}' object cannot be interpreted as an integer", res_type);
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("object of type '{type_name}' has no len()");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

/// Fast len for known-list values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_list(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST || tid == TYPE_ID_LIST_INT || tid == TYPE_ID_LIST_BOOL {
                    return MoltObject::from_int(list_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-str values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_str(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    return MoltObject::from_int(count).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-dict values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_dict(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(dict_len(dict_ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-tuple values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_tuple(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    return MoltObject::from_int(tuple_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-set values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_set(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_SET || tid == TYPE_ID_FROZENSET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_builtin(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hash = hash_bits_signed(_py, val);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_hash(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        let hash = if let Some(ptr) = obj.as_ptr() {
            hash_pointer(ptr as u64)
        } else {
            hash_pointer(val)
        };
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_id(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val);
        // For pointer-backed objects, use the raw pointer address as id
        // (matches CPython's id() which returns the memory address).
        // For inline values (ints, small floats, bools, None), use the
        // NaN-boxed bits directly — they are canonical.
        if let Some(ptr) = obj.as_ptr() {
            // Use usize to ensure non-negative id (CPython id() is always >= 0).
            // On 64-bit systems, high pointers would produce negative i64.
            let addr = ptr as usize;
            if addr <= i64::MAX as usize {
                int_bits_from_i64(_py, addr as i64)
            } else {
                // Pointer above i64::MAX — allocate BigInt.
                let big = num_bigint::BigInt::from(addr as u64);
                crate::builtins::numbers::int_bits_from_bigint(_py, big)
            }
        } else {
            int_bits_from_i64(_py, val as i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_chr(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Fast path: inline small integer in valid codepoint range.
        // Avoids BigInt allocation, error-message formatting, and (for ASCII)
        // goes straight to the interned single-char cache.
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            if !(0..=0x10FFFF).contains(&i) {
                return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
            }
            let code = i as u32;
            let mut out_bytes = Vec::with_capacity(4);
            push_wtf8_codepoint(&mut out_bytes, code);
            let out = alloc_string(_py, &out_bytes);
            if out.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out).bits();
        }

        // Slow path: BigInt / __index__ protocol.
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val, &msg) else {
            return MoltObject::none().bits();
        };
        if value.is_negative() || value > BigInt::from(0x10FFFF) {
            return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
        }
        let Some(code) = value.to_u32() else {
            return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
        };
        let mut out_bytes = Vec::with_capacity(4);
        push_wtf8_codepoint(&mut out_bytes, code);
        let out = alloc_string(_py, &out_bytes);
        if out.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_missing() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = missing_bits(_py);
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_not_implemented() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ellipsis() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { ellipsis_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pending() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::pending().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_collect(generation_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let generation = match gc_int_arg(_py, generation_bits, "generation") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if generation < 0 {
            return raise_exception::<_>(_py, "ValueError", "generation must be non-negative");
        }
        let cycle_collected = unsafe { crate::object::gc::collect_cycles(_py).collected } as i64;
        let weakref_collected = crate::object::weakref::weakref_collect_for_gc(_py) as i64;
        let collected = cycle_collected + weakref_collected;
        let mut state = gc_state().lock().unwrap();
        state.count = (0, 0, 0);
        MoltObject::from_int(collected).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_enable() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut state = gc_state().lock().unwrap();
        state.enabled = true;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_disable() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut state = gc_state().lock().unwrap();
        state.enabled = false;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_isenabled() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = gc_state().lock().unwrap();
        MoltObject::from_bool(state.enabled).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_set_threshold(th0_bits: u64, th1_bits: u64, th2_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let th0 = match gc_int_arg(_py, th0_bits, "threshold0") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let th1 = match gc_int_arg(_py, th1_bits, "threshold1") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let th2 = match gc_int_arg(_py, th2_bits, "threshold2") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut state = gc_state().lock().unwrap();
        state.thresholds = (th0, th1, th2);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_threshold() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = gc_state().lock().unwrap();
        let (th0, th1, th2) = state.thresholds;
        let th0_bits = MoltObject::from_int(th0).bits();
        let th1_bits = MoltObject::from_int(th1).bits();
        let th2_bits = MoltObject::from_int(th2).bits();
        let tuple_ptr = alloc_tuple(_py, &[th0_bits, th1_bits, th2_bits]);
        dec_ref_bits(_py, th0_bits);
        dec_ref_bits(_py, th1_bits);
        dec_ref_bits(_py, th2_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_set_debug(flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let flags = match gc_int_arg(_py, flags_bits, "flags") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut state = gc_state().lock().unwrap();
        state.debug_flags = flags;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_debug() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = gc_state().lock().unwrap();
        MoltObject::from_int(state.debug_flags).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_count() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = gc_state().lock().unwrap();
        let (c0, c1, c2) = state.count;
        let c0_bits = MoltObject::from_int(c0).bits();
        let c1_bits = MoltObject::from_int(c1).bits();
        let c2_bits = MoltObject::from_int(c2).bits();
        let tuple_ptr = alloc_tuple(_py, &[c0_bits, c1_bits, c2_bits]);
        dec_ref_bits(_py, c0_bits);
        dec_ref_bits(_py, c1_bits);
        dec_ref_bits(_py, c2_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_is_tracked(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tracked = obj_from_bits(obj_bits)
            .as_ptr()
            .map(|ptr| unsafe { crate::object::gc::gc_is_tracked(ptr) })
            .unwrap_or(false);
        MoltObject::from_bool(tracked).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getrecursionlimit() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(recursion_limit_get() as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_setrecursionlimit(limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(limit_bits);
        let limit = if let Some(value) = to_i64(obj) {
            if value < 1 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "recursion limit must be greater or equal than 1",
                );
            }
            value as usize
        } else if let Some(big_ptr) = bigint_ptr_from_bits(limit_bits) {
            let big = unsafe { bigint_ref(big_ptr) };
            if big.is_negative() {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "recursion limit must be greater or equal than 1",
                );
            }
            let Some(value) = big.to_usize() else {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            };
            value
        } else {
            let type_name = class_name_for_error(type_of_bits(_py, limit_bits));
            let msg = format!("'{type_name}' object cannot be interpreted as an integer");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let depth = RECURSION_DEPTH.with(|depth| depth.get());
        if limit <= depth {
            let msg = format!(
                "cannot set the recursion limit to {limit} at the recursion depth {depth}: the limit is too low"
            );
            return raise_exception::<_>(_py, "RecursionError", &msg);
        }
        recursion_limit_set(limit);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getargv() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut args_guard = runtime_state(_py).argv.lock().unwrap();
        if args_guard.is_empty()
            && let Some(wasi_args) = collect_wasi_argv_bytes()
            && !wasi_args.is_empty()
        {
            *args_guard = wasi_args;
        }
        // On WASM, molt_set_argv may not have been called (no C main stub).
        // Fall back to std::env::args() so WASI args are still visible.
        let env_args_storage;
        let args: &Vec<Vec<u8>> = if args_guard.is_empty() {
            env_args_storage = std::env::args().map(|s| s.into_bytes()).collect::<Vec<_>>();
            &env_args_storage
        } else {
            &args_guard
        };
        let mut elems = Vec::with_capacity(args.len());
        for arg in args.iter() {
            let ptr = alloc_string(_py, arg);
            if ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            elems.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, &elems);
        if list_ptr.is_null() {
            for bits in elems {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in elems {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_set_version_info(
    major_bits: u64,
    minor_bits: u64,
    micro_bits: u64,
    releaselevel_bits: u64,
    serial_bits: u64,
    version_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Compiler/runtime ABI note:
        // the startup stub may provide these fields either as boxed Molt ints
        // or as raw compiler immediates from typed codegen lanes.
        let decode_version_int = |bits: u64, err: &str| -> Result<i64, u64> {
            let obj = MoltObject::from_bits(bits);
            if obj.is_int() {
                return Ok(obj.as_int_unchecked());
            }
            if obj.as_ptr().is_none() {
                return Ok(bits as i64);
            }
            Err(raise_exception::<u64>(_py, "TypeError", err))
        };
        let major = match decode_version_int(major_bits, "major must be int") {
            Ok(v) => v,
            Err(err) => return err,
        };
        let minor = match decode_version_int(minor_bits, "minor must be int") {
            Ok(v) => v,
            Err(err) => return err,
        };
        let micro = match decode_version_int(micro_bits, "micro must be int") {
            Ok(v) => v,
            Err(err) => return err,
        };
        let serial = match decode_version_int(serial_bits, "serial must be int") {
            Ok(v) => v,
            Err(err) => return err,
        };
        if major < 0 || minor < 0 || micro < 0 || serial < 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "sys.version_info must be non-negative integers",
            );
        }

        let Some(release_ptr) = obj_from_bits(releaselevel_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "sys.version_info releaselevel must be str",
            );
        };
        unsafe {
            if object_type_id(release_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "sys.version_info releaselevel must be str",
                );
            }
        }
        let release_bytes = unsafe {
            std::slice::from_raw_parts(string_bytes(release_ptr), string_len(release_ptr))
        };
        let releaselevel = String::from_utf8_lossy(release_bytes).into_owned();

        let Some(version_ptr) = obj_from_bits(version_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "sys.version must be str");
        };
        unsafe {
            if object_type_id(version_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "sys.version must be str");
            }
        }
        let version_bytes = unsafe {
            std::slice::from_raw_parts(string_bytes(version_ptr), string_len(version_ptr))
        };
        let mut version = String::from_utf8_lossy(version_bytes).into_owned();

        let mut info = PythonVersionInfo {
            major,
            minor,
            micro,
            releaselevel,
            serial,
        };
        let mut info_overridden_from_env = false;
        if let Some(env_info) = env_target_python_info() {
            if env_info != info {
                info_overridden_from_env = true;
                if trace_sys_version() {
                    eprintln!(
                        "molt sys version: overriding set payload with env {}.{}.{} {} {}",
                        env_info.major,
                        env_info.minor,
                        env_info.micro,
                        env_info.releaselevel,
                        env_info.serial
                    );
                }
            }
            info = env_info;
        }

        let mut version_from_env = false;
        if let Ok(env_version) = std::env::var("MOLT_SYS_VERSION")
            && !env_version.is_empty()
        {
            version = env_version;
            version_from_env = true;
        }
        if !version_from_env && (version.is_empty() || info_overridden_from_env) {
            version = format_sys_version(&info);
        }
        if trace_sys_version() {
            eprintln!(
                "molt sys version: set called {}.{}.{} {} {}",
                info.major, info.minor, info.micro, info.releaselevel, info.serial
            );
        }

        let state = runtime_state(_py);
        let default_info = default_sys_version_info();
        {
            let mut guard = state.sys_version_info.lock().unwrap();
            if let Some(existing) = guard.as_ref()
                && existing != &info
                && existing != &default_info
            {
                return raise_exception::<_>(_py, "RuntimeError", "sys.version_info already set");
            }
            *guard = Some(info.clone());
        }
        {
            let mut guard = state.sys_version.lock().unwrap();
            if let Some(existing) = guard.as_ref()
                && existing != &version
            {
                return raise_exception::<_>(_py, "RuntimeError", "sys.version already set");
            }
            *guard = Some(version.clone());
        }
        // If the sys module already exists, keep its version metadata in sync.
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            cache.lock().unwrap().get("sys").copied()
        };
        if trace_sys_version() {
            eprintln!("molt sys version: sys module cached={}", sys_bits.is_some());
        }
        if let Some(bits) = sys_bits
            && let Some(sys_ptr) = obj_from_bits(bits).as_ptr()
        {
            unsafe {
                if crate::builtins::modules::sys_populate_version_metadata(_py, sys_ptr).is_err() {
                    return MoltObject::none().bits();
                }
                if trace_sys_version() {
                    eprintln!("molt sys version: sys dict updated");
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_version_info() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        let (info, initialized) = current_sys_version_info(state);
        if trace_sys_version() {
            eprintln!(
                "molt sys version: get info {}.{}.{} {} {} init={}",
                info.major, info.minor, info.micro, info.releaselevel, info.serial, initialized
            );
        }
        alloc_sys_version_info_tuple(_py, &info).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_version() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        let version = {
            let mut guard = state.sys_version.lock().unwrap();
            if let Some(existing) = guard.as_ref() {
                existing.clone()
            } else {
                let computed = format_sys_version(&info);
                *guard = Some(computed.clone());
                computed
            }
        };
        let ptr = alloc_string(_py, version.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_hexversion() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        MoltObject::from_int(sys_hexversion_from_info(&info)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_api_version() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_int(sys_api_version()).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_abiflags() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let abiflags = sys_abiflags();
        let ptr = alloc_string(_py, abiflags.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_implementation_payload() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        let name = sys_implementation_name();
        let cache_tag = sys_cache_tag(&name, &info);
        let hexversion_bits = MoltObject::from_int(sys_hexversion_from_info(&info)).bits();

        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let cache_tag_ptr = alloc_string(_py, cache_tag.as_bytes());
        if cache_tag_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            return MoltObject::none().bits();
        }

        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let cache_tag_bits = MoltObject::from_ptr(cache_tag_ptr).bits();
        let Some(version_bits) = alloc_sys_version_info_tuple(_py, &info) else {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, cache_tag_bits);
            return MoltObject::none().bits();
        };

        let keys_and_values: [(&[u8], u64); 4] = [
            (b"name", name_bits),
            (b"cache_tag", cache_tag_bits),
            (b"version", version_bits),
            (b"hexversion", hexversion_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = vec![name_bits, cache_tag_bits, version_bits, hexversion_bits];

        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_flags_payload() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let keys_and_values: [(&[u8], i64); 19] = [
            (b"debug", env_flag_bool("PYTHONDEBUG").unwrap_or(0)),
            (b"inspect", env_flag_bool("PYTHONINSPECT").unwrap_or(0)),
            (b"interactive", 0),
            (b"optimize", env_flag_level("PYTHONOPTIMIZE").unwrap_or(0)),
            (
                b"dont_write_bytecode",
                env_flag_bool("PYTHONDONTWRITEBYTECODE").unwrap_or(0),
            ),
            (
                b"no_user_site",
                env_flag_bool("PYTHONNOUSERSITE").unwrap_or(0),
            ),
            (b"no_site", 0),
            (b"ignore_environment", 0),
            (b"verbose", env_flag_level("PYTHONVERBOSE").unwrap_or(0)),
            (b"bytes_warning", 0),
            (b"quiet", 0),
            (b"hash_randomization", sys_flags_hash_randomization()),
            (b"isolated", 0),
            (b"dev_mode", env_flag_bool("PYTHONDEVMODE").unwrap_or(0)),
            (b"utf8_mode", env_flag_bool("PYTHONUTF8").unwrap_or(0)),
            (
                b"warn_default_encoding",
                env_flag_bool("PYTHONWARNDEFAULTENCODING").unwrap_or(0),
            ),
            (b"safe_path", env_flag_bool("PYTHONSAFEPATH").unwrap_or(0)),
            (
                b"int_max_str_digits",
                env_non_negative_i64("PYTHONINTMAXSTRDIGITS")
                    .unwrap_or(DEFAULT_SYS_FLAGS_INT_MAX_STR_DIGITS),
            ),
            (b"gil", 1),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);

        for (key, value) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(value).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_executable() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let executable = match std::env::var("MOLT_SYS_EXECUTABLE") {
            Ok(val) if !val.is_empty() => val.into_bytes(),
            _ => runtime_state(_py)
                .argv
                .lock()
                .unwrap()
                .first()
                .cloned()
                .unwrap_or_default(),
        };
        let ptr = alloc_string(_py, &executable);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated strings.
pub unsafe extern "C" fn molt_set_argv(argc: i32, argv: *const *const u8) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let mut args = Vec::new();
            if argc > 0 && !argv.is_null() {
                for idx in 0..argc {
                    let ptr = *argv.add(idx as usize);
                    if ptr.is_null() {
                        args.push(Vec::new());
                        continue;
                    }
                    let bytes = CStr::from_ptr(ptr as *const i8).to_bytes();
                    let (decoded, _) = decode_bytes_text("utf-8", "surrogateescape", bytes)
                        .expect("argv decode must succeed for utf-8+surrogateescape");
                    args.push(decoded);
                }
            }
            let trace_argv = matches!(std::env::var("MOLT_TRACE_ARGV").ok().as_deref(), Some("1"));
            if trace_argv {
                eprintln!("molt_set_argv argc={argc} argv0={:?}", args.first());
            }
            *runtime_state(_py).argv.lock().unwrap() = args;
        })
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated UTF-16 strings.
pub unsafe extern "C" fn molt_set_argv_utf16(argc: i32, argv: *const *const u16) {
    crate::with_gil_entry_nopanic!(_py, {
        let mut args = Vec::new();
        if argc > 0 && !argv.is_null() {
            for idx in 0..argc {
                let ptr = unsafe { *argv.add(idx as usize) };
                if ptr.is_null() {
                    args.push(Vec::new());
                    continue;
                }
                let mut len = 0usize;
                while unsafe { *ptr.add(len) } != 0 {
                    len += 1;
                }
                let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
                let mut raw = Vec::with_capacity(slice.len() * 2);
                for &unit in slice {
                    raw.push((unit & 0x00FF) as u8);
                    raw.push((unit >> 8) as u8);
                }
                let (decoded, _) = decode_bytes_text("utf-16-le", "surrogatepass", &raw)
                    .expect("argv decode must succeed for utf-16-le+surrogatepass");
                args.push(decoded);
            }
        }
        *runtime_state(_py).argv.lock().unwrap() = args;
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getpid() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            let pid = unsafe { crate::molt_getpid_host() };
            let pid = if pid < 0 { 0 } else { pid };
            MoltObject::from_int(pid).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_int(std::process::id() as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_raise(sig_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(sig) = to_i64(obj_from_bits(sig_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "signal number must be int");
        };
        if sig < i32::MIN as i64 || sig > i32::MAX as i64 {
            return raise_exception::<_>(_py, "ValueError", "signal number out of range");
        }
        let sig_i32 = sig as i32;
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let rc = unsafe { libc::raise(sig_i32) };
            if rc != 0 {
                return raise_exception::<_>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            if sig_i32 == 2 {
                return raise_exception::<_>(_py, "KeyboardInterrupt", "signal interrupt");
            }
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_monotonic() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_float(monotonic_now_secs(_py)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_perf_counter() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_float(monotonic_now_secs(_py)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_monotonic_ns() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        int_bits_from_bigint(_py, BigInt::from(monotonic_now_nanos(_py)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_perf_counter_ns() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        int_bits_from_bigint(_py, BigInt::from(monotonic_now_nanos(_py)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_process_time() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match process_time_duration() {
            Ok(duration) => MoltObject::from_float(duration.as_secs_f64()).bits(),
            Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_process_time_ns() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match process_time_duration() {
            Ok(duration) => int_bits_from_bigint(_py, BigInt::from(duration.as_nanos())),
            Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_time() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return raise_exception::<_>(_py, "OSError", "system time before epoch");
            }
        };
        MoltObject::from_float(now.as_secs_f64()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_time_ns() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return raise_exception::<_>(_py, "OSError", "system time before epoch");
            }
        };
        int_bits_from_bigint(_py, BigInt::from(now.as_nanos()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_sleep(secs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(secs) = to_f64(obj_from_bits(secs_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "an integer or float is required");
        };
        if secs.is_nan() {
            return raise_exception::<_>(_py, "ValueError", "Invalid value NaN (not a number)");
        }
        if secs < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "sleep length must be non-negative");
        }
        if !secs.is_finite() {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "timestamp out of range for platform time_t",
            );
        }
        let duration = match std::time::Duration::try_from_secs_f64(secs) {
            Ok(duration) => duration,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "timestamp out of range for platform time_t",
                );
            }
        };
        if !duration.is_zero() {
            let _release = GilReleaseGuard::new();
            std::thread::sleep(duration);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_localtime(secs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(secs_bits);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if obj.is_none() && require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let secs = match parse_time_seconds(_py, secs_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let secs = secs as libc::time_t;
            let tm = match localtime_tm(secs) {
                Ok(tm) => tm,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let parts = time_parts_from_tm(&tm);
            time_parts_to_tuple(_py, parts)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let offset_west = match local_offset_west_wasm(secs) {
                Ok(value) => value,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let mut parts = time_parts_from_epoch_utc(secs.saturating_sub(offset_west));
            let std_offset_west = timezone_west_wasm().unwrap_or(offset_west);
            parts.isdst = if offset_west != std_offset_west { 1 } else { 0 };
            time_parts_to_tuple(_py, parts)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_gmtime(secs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(secs_bits);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if obj.is_none() && require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let secs = match parse_time_seconds(_py, secs_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let secs = secs as libc::time_t;
            let tm = match gmtime_tm(secs) {
                Ok(tm) => tm,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let parts = time_parts_from_tm(&tm);
            time_parts_to_tuple(_py, parts)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let parts = time_parts_from_epoch_utc(secs);
            time_parts_to_tuple(_py, parts)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_strftime(fmt_bits: u64, time_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fmt_obj = obj_from_bits(fmt_bits);
        if fmt_obj.is_none() {
            return raise_exception::<_>(_py, "TypeError", "strftime() format must be str");
        }
        let Some(fmt) = string_obj_to_owned(fmt_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, fmt_bits));
            let msg = format!("strftime() format must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fmt.as_bytes().contains(&0) {
            return raise_exception::<_>(_py, "ValueError", "embedded null character");
        }
        let parts = match parse_time_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tm = match tm_from_time_parts(_py, parts) {
                Ok(tm) => tm,
                Err(bits) => return bits,
            };
            let c_fmt = match CString::new(fmt) {
                Ok(c) => c,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "embedded null character");
                }
            };
            let mut buf = vec![0u8; 128];
            loop {
                let len = unsafe {
                    #[cfg(windows)]
                    {
                        crate::windows_abi::strftime(
                            buf.as_mut_ptr() as *mut libc::c_char,
                            buf.len(),
                            c_fmt.as_ptr(),
                            &tm as *const libc::tm,
                        )
                    }
                    #[cfg(not(windows))]
                    {
                        libc::strftime(
                            buf.as_mut_ptr() as *mut libc::c_char,
                            buf.len(),
                            c_fmt.as_ptr(),
                            &tm as *const libc::tm,
                        )
                    }
                };
                if len == 0 {
                    if buf.len() >= 1_048_576 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "strftime() result too large",
                        );
                    }
                    buf.resize(buf.len() * 2, 0);
                    continue;
                }
                let slice = &buf[..len];
                let Ok(text) = std::str::from_utf8(slice) else {
                    return raise_exception::<_>(
                        _py,
                        "UnicodeError",
                        "strftime() produced non-UTF-8 output",
                    );
                };
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let out = match strftime_wasm(&fmt, parts) {
                Ok(out) => out,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_timezone() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match timezone_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match timezone_west_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_daylight() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match daylight_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match daylight_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_altzone() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match altzone_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match altzone_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_tzname() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (std_name, dst_name) = match tzname_native() {
                Ok(res) => res,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let std_ptr = alloc_string(_py, std_name.as_bytes());
            if std_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let dst_ptr = alloc_string(_py, dst_name.as_bytes());
            if dst_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(std_ptr).bits());
                return MoltObject::none().bits();
            }
            let std_bits = MoltObject::from_ptr(std_ptr).bits();
            let dst_bits = MoltObject::from_ptr(dst_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[std_bits, dst_bits]);
            dec_ref_bits(_py, std_bits);
            dec_ref_bits(_py, dst_bits);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (std_name, dst_name) = match tzname_wasm() {
                Ok(res) => res,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let std_ptr = alloc_string(_py, std_name.as_bytes());
            if std_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let dst_ptr = alloc_string(_py, dst_name.as_bytes());
            if dst_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(std_ptr).bits());
                return MoltObject::none().bits();
            }
            let std_bits = MoltObject::from_ptr(std_ptr).bits();
            let dst_bits = MoltObject::from_ptr(dst_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[std_bits, dst_bits]);
            dec_ref_bits(_py, std_bits);
            dec_ref_bits(_py, dst_bits);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_asctime(time_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parts = match parse_time_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let text = match asctime_from_parts(parts) {
            Ok(text) => text,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_mktime(time_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parts = match parse_mktime_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_float(mktime_native(parts)).bits()
        }
        #[cfg(target_arch = "wasm32")]
        {
            match mktime_wasm(parts) {
                Ok(out) => MoltObject::from_float(out).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_timegm(time_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (year, month, day, hour, minute, second) = match parse_timegm_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let days = days_from_civil(year, month, day);
        let seconds = days
            .saturating_mul(86_400)
            .saturating_add((hour as i64).saturating_mul(3600))
            .saturating_add((minute as i64).saturating_mul(60))
            .saturating_add(second as i64);
        MoltObject::from_int(seconds).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_get_clock_info(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "unknown clock");
        };
        let (name_value, implementation, resolution, monotonic, adjustable) = match name.as_str() {
            "monotonic" | "perf_counter" => (name.as_str(), "molt", 1e-9f64, true, false),
            "process_time" => ("process_time", "molt", 1e-9f64, true, false),
            "time" => {
                #[cfg(not(target_arch = "wasm32"))]
                if require_time_wall_capability::<u64>(_py).is_err() {
                    return MoltObject::none().bits();
                }
                ("time", "molt", 1e-6f64, false, true)
            }
            _ => return raise_exception::<_>(_py, "ValueError", "unknown clock"),
        };
        let name_ptr = alloc_string(_py, name_value.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let impl_ptr = alloc_string(_py, implementation.as_bytes());
        if impl_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let impl_bits = MoltObject::from_ptr(impl_ptr).bits();
        let resolution_bits = MoltObject::from_float(resolution).bits();
        let monotonic_bits = MoltObject::from_bool(monotonic).bits();
        let adjustable_bits = MoltObject::from_bool(adjustable).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                name_bits,
                impl_bits,
                resolution_bits,
                monotonic_bits,
                adjustable_bits,
            ],
        );
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, impl_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_payload(source_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let payload = traceback_payload_from_source(_py, source_bits, limit);
        traceback_payload_to_list(_py, &payload)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_components(value_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        match traceback_exception_components_payload(_py, value_bits, limit) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_chain_payload(value_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        match traceback_exception_chain_payload_bits(_py, value_bits, limit) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_source_line(filename_bits: u64, lineno_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "filename must be str");
        };
        let Some(lineno) = to_i64(obj_from_bits(lineno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "lineno must be int");
        };
        let text = traceback_source_line_native(_py, &filename, lineno);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_infer_col_offsets(line_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(line) = string_obj_to_owned(obj_from_bits(line_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "line must be str");
        };
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        let colno_bits = MoltObject::from_int(colno).bits();
        let end_colno_bits = MoltObject::from_int(end_colno).bits();
        let tuple_ptr = alloc_tuple(_py, &[colno_bits, end_colno_bits]);
        dec_ref_bits(_py, colno_bits);
        dec_ref_bits(_py, end_colno_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_caret_line(
    line_bits: u64,
    colno_bits: u64,
    end_colno_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(line) = string_obj_to_owned(obj_from_bits(line_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "line must be str");
        };
        let Some(colno) = to_i64(obj_from_bits(colno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "colno must be int");
        };
        let Some(end_colno) = to_i64(obj_from_bits(end_colno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end_colno must be int");
        };
        let out = traceback_format_caret_line_native(&line, colno, end_colno);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exception_only(exc_type_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let line = traceback_format_exception_only_line(_py, exc_type_bits, value_bits);
        traceback_lines_to_list(_py, &[line])
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exception(
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit_bits: u64,
    chain_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let chain = is_truthy(_py, obj_from_bits(chain_bits));
        let effective_exc_type_bits = if obj_from_bits(exc_type_bits).is_none() {
            traceback_exception_type_bits(_py, value_bits)
        } else {
            exc_type_bits
        };
        let effective_tb_bits = if obj_from_bits(tb_bits).is_none() {
            traceback_exception_trace_bits(value_bits)
        } else {
            tb_bits
        };
        let mut seen: HashSet<u64> = HashSet::new();
        let mut lines: Vec<String> = Vec::new();
        traceback_append_exception_chain_lines(
            _py,
            effective_exc_type_bits,
            value_bits,
            effective_tb_bits,
            limit,
            chain,
            &mut seen,
            &mut lines,
        );
        traceback_lines_to_list(_py, &lines)
    })
}

/// `traceback.format_exc(limit=None)` — format the current exception as a single
/// string.  Equivalent to `"".join(traceback.format_exception(*sys.exc_info()))`.
/// Returns the formatted string, or `"NoneType: None\n"` if no exception is active.
#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exc(limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let exc_bits_opt = exception_last_bits_noinc(_py);
        let value_bits = match exc_bits_opt {
            Some(bits) => bits,
            None => {
                // No current exception — return "NoneType: None\n"
                let s = "NoneType: None\n";
                let ptr = alloc_string(_py, s.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        };
        let exc_type_bits = traceback_exception_type_bits(_py, value_bits);
        let tb_bits = traceback_exception_trace_bits(value_bits);
        let mut seen: HashSet<u64> = HashSet::new();
        let mut lines: Vec<String> = Vec::new();
        traceback_append_exception_chain_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            true, // chain
            &mut seen,
            &mut lines,
        );
        // Join all lines into a single string
        let joined = lines.join("");
        let ptr = alloc_string(_py, joined.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_tb(tb_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let mut lines: Vec<String> = Vec::new();
        for (filename, line, name) in traceback_frames(_py, tb_bits, limit) {
            lines.push(format!("  File \"{filename}\", line {line}, in {name}\n"));
        }
        traceback_lines_to_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_stack(source_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let payload = traceback_payload_from_source(_py, source_bits, limit);
        let lines = traceback_payload_to_formatted_lines(_py, &payload);
        traceback_lines_to_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_extract_tb(tb_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let mut tuples: Vec<u64> = Vec::new();
        for (filename, lineno, name) in traceback_frames(_py, tb_bits, limit) {
            let line_text = traceback_source_line_native(_py, &filename, lineno);
            let (colno, end_colno) = traceback_infer_column_offsets(&line_text);
            let end_lineno = lineno;
            let filename_ptr = alloc_string(_py, filename.as_bytes());
            if filename_ptr.is_null() {
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let line_ptr = alloc_string(_py, line_text.as_bytes());
            if line_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
                dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
            let lineno_bits = MoltObject::from_int(lineno).bits();
            let end_lineno_bits = MoltObject::from_int(end_lineno).bits();
            let colno_bits = MoltObject::from_int(colno).bits();
            let end_colno_bits = MoltObject::from_int(end_colno).bits();
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
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_guard_enter() -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        if recursion_guard_enter() {
            1
        } else {
            raise_exception::<i64>(_py, "RecursionError", "maximum recursion depth exceeded")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_guard_exit() {
    crate::with_gil_entry_nopanic!(_py, {
        recursion_guard_exit();
    })
}

/// Lightweight recursion guard for direct calls to known functions.
/// Uses global atomics only — no TLS access on the hot path.
/// Returns 1 on success, 0 if the recursion limit is exceeded (caller must
/// handle the error).
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_enter_fast() -> i64 {
    if crate::state::recursion::recursion_guard_enter_fast() {
        1
    } else {
        0
    }
}

/// Lightweight recursion guard exit — uses global atomics only.
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_exit_fast() {
    crate::state::recursion::recursion_guard_exit_fast();
}

/// Cold-path: raise RecursionError. Only called when molt_recursion_enter_fast
/// returns 0. Acquires the GIL to create the exception object.
#[unsafe(no_mangle)]
#[cold]
pub extern "C" fn molt_raise_recursion_error() -> u64 {
    // Sync the fast global depth back to TLS before the GIL-holding code
    // reads it (traceback formatting, etc.).
    crate::state::recursion::sync_fast_depth_to_tls();
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
    })
}

#[unsafe(no_mangle)]
pub(crate) fn parse_codec_arg(
    _py: &PyToken<'_>,
    bits: u64,
    func_name: &str,
    arg_name: &str,
    default: &str,
) -> Option<String> {
    if bits == missing_bits(_py) {
        return Some(default.to_string());
    }
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not None");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    Some(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_to_bytes(
    int_bits: u64,
    length_bits: u64,
    byteorder_bits: u64,
    signed_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let length_type = class_name_for_error(type_of_bits(_py, length_bits));
        let length_msg = format!(
            "'{}' object cannot be interpreted as an integer",
            length_type
        );
        let length = index_i64_from_obj(_py, length_bits, &length_msg);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if length < 0 {
            return raise_exception::<_>(_py, "ValueError", "length argument must be non-negative");
        }
        let len = match usize::try_from(length) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(_py, "OverflowError", "length too large");
            }
        };
        let byteorder_obj = obj_from_bits(byteorder_bits);
        let Some(byteorder) = string_obj_to_owned(byteorder_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, byteorder_bits));
            let msg = format!(
                "to_bytes() argument 'byteorder' must be str, not {}",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let byteorder_norm = byteorder.to_ascii_lowercase();
        let is_little = match byteorder_norm.as_str() {
            "little" => true,
            "big" => false,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "byteorder must be either 'little' or 'big'",
                );
            }
        };
        let signed = is_truthy(_py, obj_from_bits(signed_bits));
        let value_obj = obj_from_bits(int_bits);
        let Some(value) = to_bigint(value_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, int_bits));
            let msg = format!(
                "descriptor 'to_bytes' requires a 'int' object but received '{}'",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if !signed && value.sign() == Sign::Minus {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "can't convert negative int to unsigned",
            );
        }
        let mut bytes = if signed {
            value.to_signed_bytes_be()
        } else {
            value.to_bytes_be().1
        };
        if bytes.len() > len {
            return raise_exception::<_>(_py, "OverflowError", "int too big to convert");
        }
        if bytes.len() < len {
            let pad = if signed && value.sign() == Sign::Minus {
                0xFF
            } else {
                0x00
            };
            let mut out = vec![pad; len - bytes.len()];
            out.extend_from_slice(&bytes);
            bytes = out;
        }
        if is_little {
            bytes.reverse();
        }
        let ptr = alloc_bytes(_py, &bytes);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_bytes(
    class_bits: u64,
    bytes_bits: u64,
    byteorder_bits: u64,
    signed_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let byteorder_obj = obj_from_bits(byteorder_bits);
        let Some(byteorder) = string_obj_to_owned(byteorder_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, byteorder_bits));
            let msg = format!(
                "from_bytes() argument 'byteorder' must be str, not {}",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let byteorder_norm = byteorder.to_ascii_lowercase();
        let is_little = match byteorder_norm.as_str() {
            "little" => true,
            "big" => false,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "byteorder must be either 'little' or 'big'",
                );
            }
        };
        let signed = is_truthy(_py, obj_from_bits(signed_bits));
        let bytes_obj = obj_from_bits(bytes_bits);
        let Some(bytes_ptr) = bytes_obj.as_ptr() else {
            let type_name = class_name_for_error(type_of_bits(_py, bytes_bits));
            let msg = format!("cannot convert '{}' object to bytes", type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(slice) = (unsafe { bytes_like_slice(bytes_ptr) }) else {
            let type_name = class_name_for_error(type_of_bits(_py, bytes_bits));
            let msg = format!("cannot convert '{}' object to bytes", type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let mut bytes = slice.to_vec();
        if is_little {
            bytes.reverse();
        }
        let value = if signed {
            BigInt::from_signed_bytes_be(&bytes)
        } else {
            BigInt::from_bytes_be(Sign::Plus, &bytes)
        };
        let int_bits = int_bits_from_bigint(_py, value);
        let builtins = builtin_classes(_py);
        if class_bits == builtins.int {
            return int_bits;
        }
        unsafe { call_callable1(_py, class_bits, int_bits) }
    })
}
