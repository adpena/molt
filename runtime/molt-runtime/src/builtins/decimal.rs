use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use molt_obj_model::MoltObject;

use crate::object::ops::{format_obj_str, is_truthy, string_obj_to_owned};
use crate::{
    alloc_string, alloc_tuple, bits_from_ptr, dec_ref_bits, int_bits_from_i64, obj_from_bits,
    ptr_from_bits, raise_exception, release_ptr, PyToken,
};

#[allow(non_camel_case_types)]
#[cfg(target_pointer_width = "64")]
type mpd_ssize_t = i64;
#[allow(non_camel_case_types)]
#[cfg(target_pointer_width = "64")]
type mpd_uint_t = u64;
#[allow(non_camel_case_types)]
#[cfg(target_pointer_width = "32")]
type mpd_ssize_t = i32;
#[allow(non_camel_case_types)]
#[cfg(target_pointer_width = "32")]
type mpd_uint_t = u32;

#[repr(C)]
#[derive(Clone, Copy)]
struct mpd_context_t {
    prec: mpd_ssize_t,
    emax: mpd_ssize_t,
    emin: mpd_ssize_t,
    traps: u32,
    status: u32,
    newtrap: u32,
    round: c_int,
    clamp: c_int,
    allcr: c_int,
}

#[repr(C)]
struct mpd_t {
    flags: u8,
    exp: mpd_ssize_t,
    digits: mpd_ssize_t,
    len: mpd_ssize_t,
    alloc: mpd_ssize_t,
    data: *mut mpd_uint_t,
}

extern "C" {
    fn mpd_new(ctx: *mut mpd_context_t) -> *mut mpd_t;
    fn mpd_del(dec: *mut mpd_t);
    fn mpd_qset_string(
        dec: *mut mpd_t,
        s: *const c_char,
        ctx: *const mpd_context_t,
        status: *mut u32,
    );
    fn mpd_qcopy(result: *mut mpd_t, a: *const mpd_t, status: *mut u32) -> c_int;
    fn mpd_qdiv(
        result: *mut mpd_t,
        a: *const mpd_t,
        b: *const mpd_t,
        ctx: *const mpd_context_t,
        status: *mut u32,
    );
    fn mpd_qquantize(
        result: *mut mpd_t,
        a: *const mpd_t,
        b: *const mpd_t,
        ctx: *const mpd_context_t,
        status: *mut u32,
    );
    fn mpd_qreduce(
        result: *mut mpd_t,
        a: *const mpd_t,
        ctx: *const mpd_context_t,
        status: *mut u32,
    );
    fn mpd_qcompare(
        result: *mut mpd_t,
        a: *const mpd_t,
        b: *const mpd_t,
        ctx: *const mpd_context_t,
        status: *mut u32,
    ) -> c_int;
    fn mpd_compare_total(result: *mut mpd_t, a: *const mpd_t, b: *const mpd_t) -> c_int;
    fn mpd_qexp(result: *mut mpd_t, a: *const mpd_t, ctx: *const mpd_context_t, status: *mut u32);
    fn mpd_to_sci(dec: *const mpd_t, fmt: c_int) -> *mut c_char;
    static mut mpd_free: Option<unsafe extern "C" fn(*mut c_void)>;
    fn mpd_qsetprec(ctx: *mut mpd_context_t, prec: mpd_ssize_t) -> c_int;
    fn mpd_qsetround(ctx: *mut mpd_context_t, round: c_int) -> c_int;
}

const MPD_CLAMPED: u32 = 0x00000001;
const MPD_CONVERSION_SYNTAX: u32 = 0x00000002;
const MPD_DIVISION_BY_ZERO: u32 = 0x00000004;
const MPD_DIVISION_IMPOSSIBLE: u32 = 0x00000008;
const MPD_DIVISION_UNDEFINED: u32 = 0x00000010;
const MPD_FPU_ERROR: u32 = 0x00000020;
const MPD_INEXACT: u32 = 0x00000040;
const MPD_INVALID_CONTEXT: u32 = 0x00000080;
const MPD_INVALID_OPERATION: u32 = 0x00000100;
const MPD_MALLOC_ERROR: u32 = 0x00000200;
const MPD_NOT_IMPLEMENTED: u32 = 0x00000400;
const MPD_OVERFLOW: u32 = 0x00000800;
const MPD_ROUNDED: u32 = 0x00001000;
const MPD_SUBNORMAL: u32 = 0x00002000;
const MPD_UNDERFLOW: u32 = 0x00004000;

const MPD_IEEE_INVALID_OPERATION: u32 = MPD_CONVERSION_SYNTAX
    | MPD_DIVISION_IMPOSSIBLE
    | MPD_DIVISION_UNDEFINED
    | MPD_FPU_ERROR
    | MPD_INVALID_CONTEXT
    | MPD_INVALID_OPERATION
    | MPD_MALLOC_ERROR;

#[allow(dead_code)]
const MPD_ROUND_UP: c_int = 0;
#[allow(dead_code)]
const MPD_ROUND_DOWN: c_int = 1;
#[allow(dead_code)]
const MPD_ROUND_CEILING: c_int = 2;
#[allow(dead_code)]
const MPD_ROUND_FLOOR: c_int = 3;
#[allow(dead_code)]
const MPD_ROUND_HALF_UP: c_int = 4;
#[allow(dead_code)]
const MPD_ROUND_HALF_DOWN: c_int = 5;
const MPD_ROUND_HALF_EVEN: c_int = 6;
#[allow(dead_code)]
const MPD_ROUND_05UP: c_int = 7;

const DECIMAL_DEFAULT_PREC: mpd_ssize_t = 28;
const DECIMAL_DEFAULT_EMAX: mpd_ssize_t = 999_999;
const DECIMAL_DEFAULT_EMIN: mpd_ssize_t = -999_999;
const DECIMAL_DEFAULT_TRAPS: u32 = MPD_IEEE_INVALID_OPERATION | MPD_DIVISION_BY_ZERO | MPD_OVERFLOW;

thread_local! {
    static DECIMAL_CONTEXT: RefCell<*mut DecimalContextHandle> = const { RefCell::new(ptr::null_mut()) };
}

struct DecimalContextHandle {
    ctx: mpd_context_t,
    capitals: c_int,
    refs: usize,
}

struct DecimalHandle {
    dec: *mut mpd_t,
}

impl Drop for DecimalHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.dec.is_null() {
                mpd_del(self.dec);
            }
        }
    }
}

fn default_context() -> mpd_context_t {
    mpd_context_t {
        prec: DECIMAL_DEFAULT_PREC,
        emax: DECIMAL_DEFAULT_EMAX,
        emin: DECIMAL_DEFAULT_EMIN,
        traps: DECIMAL_DEFAULT_TRAPS,
        status: 0,
        newtrap: 0,
        round: MPD_ROUND_HALF_EVEN,
        clamp: 0,
        allcr: 1,
    }
}

fn ensure_current_context() -> *mut DecimalContextHandle {
    DECIMAL_CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_null() {
            let handle = Box::new(DecimalContextHandle {
                ctx: default_context(),
                capitals: 1,
                refs: 1,
            });
            *slot = Box::into_raw(handle);
        }
        *slot
    })
}

fn context_inc(ptr: *mut DecimalContextHandle) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (*ptr).refs = (*ptr).refs.saturating_add(1);
    }
}

fn context_dec(ptr: *mut DecimalContextHandle) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        if (*ptr).refs <= 1 {
            release_ptr(ptr as *mut u8);
            drop(Box::from_raw(ptr));
        } else {
            (*ptr).refs -= 1;
        }
    }
}

fn context_ptr_from_bits(bits: u64) -> Option<*mut DecimalContextHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        None
    } else {
        Some(ptr as *mut DecimalContextHandle)
    }
}

fn decimal_handle_from_bits(bits: u64) -> Option<&'static mut DecimalHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut DecimalHandle) })
}

fn decimal_new_for_context(
    _py: &PyToken<'_>,
    ctx: &mut DecimalContextHandle,
) -> Result<*mut mpd_t, u64> {
    let dec = unsafe { mpd_new(&mut ctx.ctx) };
    if dec.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "decimal allocation failed",
        ));
    }
    Ok(dec)
}

fn decimal_signal_name(flags: u32) -> &'static str {
    if flags & MPD_INVALID_OPERATION != 0
        || flags & MPD_CONVERSION_SYNTAX != 0
        || flags & MPD_DIVISION_IMPOSSIBLE != 0
        || flags & MPD_DIVISION_UNDEFINED != 0
        || flags & MPD_INVALID_CONTEXT != 0
        || flags & MPD_NOT_IMPLEMENTED != 0
    {
        return "InvalidOperation";
    }
    if flags & MPD_DIVISION_BY_ZERO != 0 {
        return "DivisionByZero";
    }
    if flags & MPD_OVERFLOW != 0 {
        return "Overflow";
    }
    if flags & MPD_UNDERFLOW != 0 {
        return "Underflow";
    }
    if flags & MPD_SUBNORMAL != 0 {
        return "Subnormal";
    }
    if flags & MPD_INEXACT != 0 {
        return "Inexact";
    }
    if flags & MPD_ROUNDED != 0 {
        return "Rounded";
    }
    if flags & MPD_CLAMPED != 0 {
        return "Clamped";
    }
    "InvalidOperation"
}

fn apply_status(_py: &PyToken<'_>, ctx: &mut DecimalContextHandle, status: u32) -> Result<(), u64> {
    if status == 0 {
        return Ok(());
    }
    ctx.ctx.status |= status;
    if status & MPD_MALLOC_ERROR != 0 {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "decimal allocation failed",
        ));
    }
    let trapped = ctx.ctx.traps & status;
    if trapped != 0 {
        let name = decimal_signal_name(trapped);
        return Err(raise_exception::<u64>(_py, name, "decimal signal"));
    }
    Ok(())
}

fn decimal_handle_from_str(
    _py: &PyToken<'_>,
    ctx: &mut DecimalContextHandle,
    value: &str,
) -> Result<u64, u64> {
    let cstr = CString::new(value)
        .map_err(|_| raise_exception::<u64>(_py, "ValueError", "invalid decimal"))?;
    let dec = decimal_new_for_context(_py, ctx)?;
    let mut status: u32 = 0;
    unsafe {
        mpd_qset_string(dec, cstr.as_ptr(), &ctx.ctx, &mut status);
    }
    if let Err(bits) = apply_status(_py, ctx, status) {
        unsafe {
            mpd_del(dec);
        }
        return Err(bits);
    }
    let handle = Box::new(DecimalHandle { dec });
    Ok(bits_from_ptr(Box::into_raw(handle) as *mut u8))
}

fn decimal_to_string(
    _py: &PyToken<'_>,
    dec: &DecimalHandle,
    capitals: c_int,
) -> Result<String, u64> {
    let raw = unsafe { mpd_to_sci(dec.dec, if capitals != 0 { 1 } else { 0 }) };
    if raw.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "decimal to string failed",
        ));
    }
    let text = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    unsafe {
        if let Some(free_fn) = mpd_free {
            free_fn(raw as *mut c_void);
        } else {
            libc::free(raw as *mut c_void);
        }
    }
    Ok(text)
}

enum DecimalExponent {
    Int(i64),
    Nan,
    Snan,
    Inf,
}

fn parse_decimal_tuple(text: &str) -> (i64, Vec<i64>, DecimalExponent) {
    let trimmed = text.trim();
    let (sign, mut rest) = if let Some(stripped) = trimmed.strip_prefix('-') {
        (1, stripped)
    } else if let Some(stripped) = trimmed.strip_prefix('+') {
        (0, stripped)
    } else {
        (0, trimmed)
    };
    match rest {
        "NaN" => return (sign, Vec::new(), DecimalExponent::Nan),
        "sNaN" => return (sign, Vec::new(), DecimalExponent::Snan),
        "Infinity" => return (sign, vec![0], DecimalExponent::Inf),
        _ => {}
    }
    let mut exp_val: i64 = 0;
    if let Some(idx) = rest.find(['e', 'E']) {
        let (base, exp) = rest.split_at(idx);
        rest = base;
        let exp_str = exp[1..].trim();
        exp_val = exp_str.parse::<i64>().unwrap_or(0);
    }
    let mut digits = Vec::new();
    let mut frac_len: i64 = 0;
    let mut in_frac = false;
    for ch in rest.chars() {
        if ch == '.' {
            in_frac = true;
            continue;
        }
        if ch.is_ascii_digit() {
            digits.push((ch as u8 - b'0') as i64);
            if in_frac {
                frac_len += 1;
            }
        }
    }
    let exponent = DecimalExponent::Int(exp_val - frac_len);
    (sign, digits, exponent)
}

fn exponent_bits(_py: &PyToken<'_>, exponent: DecimalExponent) -> u64 {
    match exponent {
        DecimalExponent::Int(val) => int_bits_from_i64(_py, val),
        DecimalExponent::Nan => {
            let ptr = alloc_string(_py, b"n");
            MoltObject::from_ptr(ptr).bits()
        }
        DecimalExponent::Snan => {
            let ptr = alloc_string(_py, b"N");
            MoltObject::from_ptr(ptr).bits()
        }
        DecimalExponent::Inf => {
            let ptr = alloc_string(_py, b"F");
            MoltObject::from_ptr(ptr).bits()
        }
    }
}

fn decimal_tuple_bits(_py: &PyToken<'_>, text: &str) -> u64 {
    let (sign, digits, exponent) = parse_decimal_tuple(text);
    let sign_bits = int_bits_from_i64(_py, sign);
    let mut digit_bits = Vec::with_capacity(digits.len());
    for digit in digits {
        digit_bits.push(int_bits_from_i64(_py, digit));
    }
    let digits_ptr = alloc_tuple(_py, &digit_bits);
    if digits_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let digits_bits = MoltObject::from_ptr(digits_ptr).bits();
    let exp_bits = exponent_bits(_py, exponent);
    let tuple_ptr = alloc_tuple(_py, &[sign_bits, digits_bits, exp_bits]);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, digits_bits);
        return MoltObject::none().bits();
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    dec_ref_bits(_py, digits_bits);
    tuple_bits
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = Box::new(DecimalContextHandle {
            ctx: default_context(),
            capitals: 1,
            refs: 1,
        });
        bits_from_ptr(Box::into_raw(handle) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_get_current() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ensure_current_context();
        context_inc(ptr);
        bits_from_ptr(ptr as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_set_current(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(new_ptr) = context_ptr_from_bits(ctx_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal context");
        };
        context_inc(new_ptr);
        let old_ptr = DECIMAL_CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let old = *slot;
            *slot = new_ptr;
            old
        });
        if !old_ptr.is_null() {
            context_inc(old_ptr);
            context_dec(old_ptr);
            return bits_from_ptr(old_ptr as *mut u8);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_copy(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let ctx = unsafe { &*ctx_ptr };
        let handle = Box::new(DecimalContextHandle {
            ctx: ctx.ctx,
            capitals: ctx.capitals,
            refs: 1,
        });
        bits_from_ptr(Box::into_raw(handle) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_drop(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(ctx_bits) as *mut DecimalContextHandle;
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        context_dec(ptr);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_get_prec(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let ctx = unsafe { &*ctx_ptr };
        int_bits_from_i64(_py, ctx.ctx.prec as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_set_prec(ctx_bits: u64, prec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value_obj = obj_from_bits(prec_bits);
        let Some(prec) = value_obj.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "prec must be int");
        };
        let ok = unsafe { mpd_qsetprec(&mut (*ctx_ptr).ctx, prec as mpd_ssize_t) };
        if ok == 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid decimal precision");
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_get_rounding(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let ctx = unsafe { &*ctx_ptr };
        int_bits_from_i64(_py, ctx.ctx.round as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_set_rounding(ctx_bits: u64, round_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value_obj = obj_from_bits(round_bits);
        let Some(round) = value_obj.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "rounding must be int");
        };
        let ok = unsafe { mpd_qsetround(&mut (*ctx_ptr).ctx, round as c_int) };
        if ok == 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid rounding mode");
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_clear_flags(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        unsafe {
            (*ctx_ptr).ctx.status = 0;
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_get_flag(ctx_bits: u64, flag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value = obj_from_bits(flag_bits);
        let Some(flag) = value.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let status = unsafe { (*ctx_ptr).ctx.status };
        MoltObject::from_bool((status & flag as u32) != 0).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_set_flag(
    ctx_bits: u64,
    flag_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value = obj_from_bits(flag_bits);
        let Some(flag) = value.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let set = is_truthy(_py, obj_from_bits(value_bits));
        unsafe {
            if set {
                (*ctx_ptr).ctx.status |= flag as u32;
            } else {
                (*ctx_ptr).ctx.status &= !(flag as u32);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_get_trap(ctx_bits: u64, flag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value = obj_from_bits(flag_bits);
        let Some(flag) = value.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let traps = unsafe { (*ctx_ptr).ctx.traps };
        MoltObject::from_bool((traps & flag as u32) != 0).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_context_set_trap(
    ctx_bits: u64,
    flag_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let value = obj_from_bits(flag_bits);
        let Some(flag) = value.as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let set = is_truthy(_py, obj_from_bits(value_bits));
        unsafe {
            if set {
                (*ctx_ptr).ctx.traps |= flag as u32;
            } else {
                (*ctx_ptr).ctx.traps &= !(flag as u32);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_from_str(ctx_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let obj = obj_from_bits(value_bits);
        let Some(text) = string_obj_to_owned(obj) else {
            return raise_exception::<u64>(_py, "TypeError", "decimal value must be str");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        match decimal_handle_from_str(_py, ctx, text.trim()) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_from_int(ctx_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let obj = obj_from_bits(value_bits);
        let text = format_obj_str(_py, obj);
        let ctx = unsafe { &mut *ctx_ptr };
        match decimal_handle_from_str(_py, ctx, text.trim()) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_clone(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx_ptr = ensure_current_context();
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qcopy(result, handle.dec, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe {
                mpd_del(result);
            }
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_drop(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(value_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        unsafe {
            drop(Box::from_raw(ptr as *mut DecimalHandle));
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_to_string(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx_ptr = ensure_current_context();
        let capitals = unsafe { (*ctx_ptr).capitals };
        let text = match decimal_to_string(_py, handle, capitals) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_as_tuple(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx_ptr = ensure_current_context();
        let capitals = unsafe { (*ctx_ptr).capitals };
        let text = match decimal_to_string(_py, handle, capitals) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        decimal_tuple_bits(_py, &text)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_to_float(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx_ptr = ensure_current_context();
        let capitals = unsafe { (*ctx_ptr).capitals };
        let text = match decimal_to_string(_py, handle, capitals) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let parsed = match text.as_str() {
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            "NaN" | "sNaN" => f64::NAN,
            _ => text.parse::<f64>().unwrap_or(f64::NAN),
        };
        MoltObject::from_float(parsed).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_div(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qdiv(result, a.dec, b.dec, &ctx.ctx, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe { mpd_del(result) };
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_quantize(ctx_bits: u64, a_bits: u64, exp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(exp) = decimal_handle_from_bits(exp_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qquantize(result, a.dec, exp.dec, &ctx.ctx, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe { mpd_del(result) };
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_compare(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qcompare(result, a.dec, b.dec, &ctx.ctx, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe { mpd_del(result) };
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_compare_total(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx_ptr = ensure_current_context();
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            mpd_compare_total(result, a.dec, b.dec);
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_normalize(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qreduce(result, a.dec, &ctx.ctx, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe { mpd_del(result) };
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_decimal_exp(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        let result = match decimal_new_for_context(_py, ctx) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let mut status: u32 = 0;
        unsafe {
            mpd_qexp(result, a.dec, &ctx.ctx, &mut status);
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            unsafe { mpd_del(result) };
            return bits;
        }
        let boxed = Box::new(DecimalHandle { dec: result });
        bits_from_ptr(Box::into_raw(boxed) as *mut u8)
    })
}
