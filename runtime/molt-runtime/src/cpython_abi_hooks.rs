//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use std::ffi::CStr;
use std::os::raw::c_int;
use std::ptr;

use molt_cpython_abi::abi_types::{
    METH_CLASS, METH_COEXIST, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O,
    METH_STATIC, METH_VARARGS, MoltTypeTag, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionWithKeywords, PyModule_Type, PyModuleDef,
    PyModuleDef_Type, PyObject, PyTypeObject,
};
use molt_cpython_abi::{
    BorrowedHandleResult, EXCEPTION_SNAPSHOT_ARGS, EXCEPTION_SNAPSHOT_CAUSE,
    EXCEPTION_SNAPSHOT_CONTEXT, EXCEPTION_SNAPSHOT_DICT, EXCEPTION_SNAPSHOT_NOTES,
    EXCEPTION_SNAPSHOT_TRACEBACK, ExceptionSnapshot, MoltBufferView as AbiMoltBufferView,
    OwnedHandleResult, RuntimeHooks,
};
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;

use crate::builtins::containers::{dict_len, dict_order, list_len, tuple_len};
use crate::builtins::numbers::{
    INT_BYTES_INVALID, bigint_from_bytes, bigint_from_f64_trunc, bigint_num_bits,
    bigint_ptr_from_bits, bigint_ref, bigint_to_bytes, int_bits_from_bigint, int_bits_from_i64,
    int_bits_from_i128, to_bigint, to_i64,
};
use crate::concurrency::gil::with_gil;
use crate::concurrency::{GilGuard, GilReleaseGuard, gil_owned_by_current_thread};
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_filled,
    alloc_list_with_capacity, alloc_module_obj, alloc_string, alloc_tuple_uninitialized,
};
use crate::object::layout::{
    function_set_call_target_ptr, function_set_dict_bits, function_set_trampoline_ptr,
    module_dict_bits,
};
use crate::object::ops::{
    dict_del_in_place, dict_get_in_place, dict_get_str_bytes_borrowed, dict_set_in_place,
};
use crate::object::type_ids::{
    TYPE_ID_BIGINT, TYPE_ID_BYTES, TYPE_ID_COMPLEX, TYPE_ID_DICT, TYPE_ID_FROZENSET, TYPE_ID_LIST,
    TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE,
};
use crate::object::{
    HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, bytes_data, bytes_len, dec_ref_bits, header_from_obj_ptr,
    inc_ref_bits, object_type_id, string_bytes, string_len,
};

// ─── Hook implementations ─────────────────────────────────────────────────

fn abi_buffer_view_from_runtime(view: crate::MoltBufferView) -> AbiMoltBufferView {
    unsafe { std::mem::transmute::<crate::MoltBufferView, AbiMoltBufferView>(view) }
}

thread_local! {
    static ABI_GIL_ENSURE_GUARDS: std::cell::RefCell<Vec<GilGuard>> = const { std::cell::RefCell::new(Vec::new()) };
    static ABI_GIL_RELEASE_GUARDS: std::cell::RefCell<Vec<GilReleaseGuard>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[inline]
fn owned_result_from_pending(bits: u64) -> OwnedHandleResult {
    if with_gil(|_py| crate::exception_pending(&_py)) {
        OwnedHandleResult::error()
    } else if bits == 0 {
        with_gil(|_py| {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "runtime owned-result hook returned reserved zero without an exception",
            );
        });
        OwnedHandleResult::error()
    } else {
        OwnedHandleResult::ok(bits)
    }
}

unsafe extern "C" fn hook_gil_ensure() -> c_int {
    let was_held = gil_owned_by_current_thread();
    ABI_GIL_ENSURE_GUARDS.with(|guards| guards.borrow_mut().push(GilGuard::new()));
    c_int::from(was_held)
}

unsafe extern "C" fn hook_gil_leave(_state: c_int) {
    ABI_GIL_ENSURE_GUARDS.with(|guards| {
        let guard = guards
            .borrow_mut()
            .pop()
            .expect("PyGILState_Release without matching PyGILState_Ensure");
        drop(guard);
    });
}

unsafe extern "C" fn hook_gil_release() {
    ABI_GIL_RELEASE_GUARDS.with(|guards| guards.borrow_mut().push(GilReleaseGuard::new()));
}

unsafe extern "C" fn hook_gil_restore() {
    ABI_GIL_RELEASE_GUARDS.with(|guards| {
        let guard = guards
            .borrow_mut()
            .pop()
            .expect("PyEval_RestoreThread without matching PyEval_SaveThread");
        drop(guard);
    });
}

unsafe extern "C" fn hook_gil_check() -> c_int {
    c_int::from(gil_owned_by_current_thread())
}

fn runtime_buffer_view_from_abi(view: AbiMoltBufferView) -> crate::MoltBufferView {
    unsafe { std::mem::transmute::<AbiMoltBufferView, crate::MoltBufferView>(view) }
}

unsafe extern "C" fn hook_alloc_str(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_string(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            // NaN-box the heap pointer so the bridge round-trip via
            // PyObject* -> trailing-bits read recovers a value the runtime's
            // `obj.as_ptr()` recognises as a heap pointer (see
            // `MoltObject::from_ptr` for the canonical encoding).
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_bytes(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_bytes(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_int_from_i64(value: i64) -> u64 {
    with_gil(|_py| int_bits_from_i64(&_py, value))
}

unsafe extern "C" fn hook_int_from_u64(value: u64) -> u64 {
    with_gil(|_py| int_bits_from_i128(&_py, value as i128))
}

unsafe extern "C" fn hook_int_as_i64(bits: u64) -> i64 {
    with_gil(|_py| to_i64(MoltObject::from_bits(bits)).unwrap_or(-1))
}

unsafe extern "C" fn hook_int_as_i64_checked(bits: u64, out: *mut i64) -> i32 {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| match to_i64(MoltObject::from_bits(bits)) {
        Some(value) => {
            unsafe {
                *out = value;
            }
            0
        }
        None => -1,
    })
}

unsafe extern "C" fn hook_int_as_u64_checked(bits: u64, out: *mut u64) -> i32 {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = to_i64(obj) {
            if value < 0 {
                return -1;
            }
            unsafe {
                *out = value as u64;
            }
            return 0;
        }
        if let Some(value) = to_bigint(obj).and_then(|value| value.to_u64()) {
            unsafe {
                *out = value;
            }
            return 0;
        }
        -1
    })
}

unsafe extern "C" fn hook_int_as_u64_mask(bits: u64, width: u32, out: *mut u64) -> i32 {
    if out.is_null() || width == 0 || width > 64 {
        return -1;
    }
    with_gil(|_py| {
        let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
            return -1;
        };
        let modulus = BigInt::from(1u8) << width;
        let masked = ((value % &modulus) + &modulus) % &modulus;
        let Some(masked) = masked.to_u64() else {
            return -1;
        };
        unsafe { *out = masked };
        0
    })
}

unsafe extern "C" fn hook_int_from_bytes(
    data: *const u8,
    len: usize,
    little_endian: c_int,
    signed: c_int,
) -> u64 {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if data.is_null() && len != 0 {
        return 0;
    }
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let value = bigint_from_bytes(bytes, little_endian != 0, signed != 0);
    with_gil(|_py| int_bits_from_bigint(&_py, value))
}

unsafe extern "C" fn hook_int_from_digits(
    digits: *const u8,
    len: usize,
    base: u32,
    negative: c_int,
) -> u64 {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if digits.is_null() && len != 0 || !(2..=36).contains(&base) {
        return 0;
    }
    let digits = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(digits, len) }
    };
    let sign = if negative != 0 {
        Sign::Minus
    } else {
        Sign::Plus
    };
    let Some(value) = BigInt::from_radix_be(sign, digits, base) else {
        return 0;
    };
    with_gil(|_py| int_bits_from_bigint(&_py, value))
}

unsafe extern "C" fn hook_int_from_f64_trunc(value: f64) -> u64 {
    if !value.is_finite() {
        return 0;
    }
    with_gil(|_py| int_bits_from_bigint(&_py, bigint_from_f64_trunc(value)))
}

unsafe extern "C" fn hook_int_sign(bits: u64) -> c_int {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = obj.as_int() {
            return value.signum() as c_int;
        }
        if let Some(value) = obj.as_bool() {
            return value as c_int;
        }
        bigint_ptr_from_bits(bits)
            .map(|ptr| unsafe { bigint_ref(ptr) }.sign())
            .map_or(0, |sign| match sign {
                Sign::Minus => -1,
                Sign::NoSign => 0,
                Sign::Plus => 1,
            })
    })
}

unsafe extern "C" fn hook_int_signed_byte_width(bits: u64, out: *mut usize) -> c_int {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let obj = MoltObject::from_bits(bits);
        let width = if let Some(value) = obj.as_int() {
            let significant = if value >= 0 {
                65 - value.leading_zeros() as usize
            } else {
                65 - (!value).leading_zeros() as usize
            };
            significant.div_ceil(8)
        } else if let Some(ptr) = bigint_ptr_from_bits(bits) {
            let value = unsafe { bigint_ref(ptr) };
            let bit_len = usize::try_from(value.bits()).unwrap_or(usize::MAX);
            match value.sign() {
                Sign::NoSign => 1,
                Sign::Plus => bit_len.saturating_add(1).div_ceil(8).max(1),
                Sign::Minus => {
                    let exact_power = value.magnitude().trailing_zeros()
                        == Some(value.magnitude().bits().saturating_sub(1));
                    let base = bit_len.div_ceil(8).max(1);
                    if bit_len % 8 == 0 && !exact_power {
                        base.saturating_add(1)
                    } else {
                        base
                    }
                }
            }
        } else {
            return -1;
        };
        unsafe { *out = width };
        0
    })
}

unsafe extern "C" fn hook_int_to_bytes(
    bits: u64,
    data: *mut u8,
    len: usize,
    little_endian: c_int,
    signed: c_int,
) -> c_int {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if data.is_null() && len != 0 {
        return INT_BYTES_INVALID;
    }
    with_gil(|_py| {
        let out = if len == 0 {
            &mut [][..]
        } else {
            unsafe { std::slice::from_raw_parts_mut(data, len) }
        };
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return bigint_to_bytes(
                unsafe { bigint_ref(ptr) },
                out,
                little_endian != 0,
                signed != 0,
            );
        }
        let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
            return INT_BYTES_INVALID;
        };
        bigint_to_bytes(&value, out, little_endian != 0, signed != 0)
    })
}

unsafe extern "C" fn hook_int_num_bits(bits: u64, out: *mut usize) -> c_int {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let inline;
        let value = if let Some(ptr) = bigint_ptr_from_bits(bits) {
            unsafe { bigint_ref(ptr) }
        } else {
            let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
                return -1;
            };
            inline = value;
            &inline
        };
        let Some(num_bits) = bigint_num_bits(value) else {
            return -1;
        };
        unsafe { *out = num_bits };
        0
    })
}

unsafe extern "C" fn hook_int_max_str_digits() -> usize {
    with_gil(|_py| crate::builtins::sys_ext::current_int_max_str_digits(&_py))
}

unsafe extern "C" fn hook_alloc_list() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_list_with_capacity(&_py, &[], 8);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_list_presized(len: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_list_filled(&_py, len, MoltObject::none());
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[inline]
fn is_list_type_id(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
}

unsafe fn list_item_bits(ptr: *mut u8, i: usize) -> Option<u64> {
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_LIST => unsafe { crate::object::seq_access::item(ptr, i) },
        TYPE_ID_LIST_INT => unsafe { crate::object::layout::list_int_vec_ref(ptr) }
            .as_slice()
            .get(i)
            .copied()
            .map(|value| MoltObject::from_int(value).bits()),
        TYPE_ID_LIST_BOOL => unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
            .as_slice()
            .get(i)
            .copied()
            .map(|value| MoltObject::from_bool(value != 0).bits()),
        _ => None,
    }
}

unsafe fn list_bits_snapshot(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<Vec<u64>> {
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == TYPE_ID_LIST {
        let copied = unsafe {
            crate::object::seq_access::with_borrowed(ptr, |source| {
                let mut out = Vec::new();
                if out.try_reserve_exact(source.len()).is_err() {
                    return None;
                }
                out.extend_from_slice(source);
                Some(out)
            })
        };
        if copied.is_none() {
            let _ =
                crate::raise_exception::<u64>(_py, "MemoryError", "list slice allocation failed");
        }
        return copied;
    }
    let len = match type_id {
        TYPE_ID_LIST_INT => unsafe { crate::object::layout::list_int_vec_ref(ptr) }.len(),
        TYPE_ID_LIST_BOOL => unsafe { crate::object::layout::list_bool_vec_ref(ptr) }.len(),
        _ => return None,
    };
    let mut out = Vec::new();
    if out.try_reserve_exact(len).is_err() {
        let _ = crate::raise_exception::<u64>(_py, "MemoryError", "list slice allocation failed");
        return None;
    }
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_LIST_INT => out.extend(
            unsafe { crate::object::layout::list_int_vec_ref(ptr) }
                .iter()
                .copied()
                .map(|value| MoltObject::from_int(value).bits()),
        ),
        TYPE_ID_LIST_BOOL => out.extend(
            unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
                .iter()
                .copied()
                .map(|value| MoltObject::from_bool(value != 0).bits()),
        ),
        _ => unreachable!(),
    }
    Some(out)
}

unsafe extern "C" fn hook_list_append(
    list_bits: u64,
    item_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> i32 {
    // Keep representation selection, promotion, allocation accounting, and
    // element refcounting in the runtime's single list-append authority.
    if crate::object::ops_list::molt_list_append_with_projection(list_bits, item_bits, item_ptr) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn hook_list_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if !is_list_type_id(unsafe { object_type_id(ptr) }) {
        return 0;
    }
    unsafe { list_len(ptr) }
}

unsafe extern "C" fn hook_list_item(bits: u64, i: usize) -> BorrowedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return BorrowedHandleResult::missing(),
    };
    unsafe { list_item_bits(ptr, i) }
        .map(BorrowedHandleResult::ok)
        .unwrap_or_else(BorrowedHandleResult::missing)
}

/// Indexed list store backing `PyList_SetItem`/`PyList_SET_ITEM`. Writes the
/// previous occupant's bits into `*out_old` (so the ABI can release the CPython
/// stolen-ref / `Py_SETREF` old reference) and returns 1 on success, 0 when `i`
/// is out of range or the object is not a list. O(1), allocation-free.
unsafe extern "C" fn hook_list_set(list_bits: u64, i: usize, val_bits: u64) -> OwnedHandleResult {
    let obj = MoltObject::from_bits(list_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return OwnedHandleResult::error(),
    };
    with_gil(|_py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return OwnedHandleResult::error();
        }
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return OwnedHandleResult::error();
        }
        unsafe { crate::object::list_mutation::replace_one_runtime_only(&_py, ptr, i, val_bits) }
            .map(OwnedHandleResult::ok)
            .unwrap_or_else(OwnedHandleResult::error)
    })
}

/// Insert before (clamped) index `where_` — routes to the runtime `PyList_Insert`
/// (`ins1`) authority so the shift semantics are the single source of truth.
unsafe extern "C" fn hook_list_insert(
    list_bits: u64,
    where_: isize,
    item_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> i32 {
    let Some(ptr) = MoltObject::from_bits(list_bits).as_ptr() else {
        return -1;
    };
    with_gil(|py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return -1;
        }
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&py, ptr) };
        if unsafe {
            crate::object::ops_list::insert_at_native_index_with_projection(
                &py,
                ptr,
                where_ as i64,
                item_bits,
                item_ptr,
            )
        } {
            0
        } else {
            -1
        }
    })
}

/// Sort in place — routes to the runtime `PyList_Sort` (comparison authority).
unsafe extern "C" fn hook_list_sort(list_bits: u64) -> i32 {
    crate::c_api::PyList_Sort(list_bits)
}

/// Reverse in place — routes to the runtime `PyList_Reverse` authority.
unsafe extern "C" fn hook_list_reverse(list_bits: u64) -> i32 {
    crate::c_api::PyList_Reverse(list_bits)
}

/// Replace `list[ilow:ihigh]` with the elements of `itemlist_bits` (a list/tuple,
/// or 0 to delete the slice), growing/shrinking the backing vector via
/// `Vec::splice`. The replacement is cloned before the mutable borrow so a
/// self-slice (`a[i:j] = a`) is safe. Returns -1 for a non-list receiver or a
/// non-list/tuple itemlist.
unsafe extern "C" fn hook_list_set_slice(
    list_bits: u64,
    ilow: isize,
    ihigh: isize,
    itemlist_bits: u64,
    future_pointers: *const *mut molt_cpython_abi::abi_types::PyObject,
    future_len: usize,
) -> i32 {
    let ptr = match MoltObject::from_bits(list_bits).as_ptr() {
        Some(p) => p,
        None => return -1,
    };
    with_gil(|_py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return -1;
        }
        let replacement: Vec<u64> = if itemlist_bits == 0 {
            Vec::new()
        } else {
            match MoltObject::from_bits(itemlist_bits).as_ptr() {
                Some(ip) if unsafe { object_type_id(ip) } == TYPE_ID_TUPLE => {
                    let out = unsafe {
                        crate::object::seq_access::with_immutable_tuple_slice(ip, |source| {
                            let mut out = Vec::new();
                            if out.try_reserve_exact(source.len()).is_err() {
                                return None;
                            }
                            out.extend_from_slice(source);
                            Some(out)
                        })
                    }
                    .flatten();
                    let Some(out) = out else {
                        let _ = crate::raise_exception::<u64>(
                            &_py,
                            "MemoryError",
                            "list slice allocation failed",
                        );
                        return -1;
                    };
                    out
                }
                Some(ip) => match unsafe { list_bits_snapshot(&_py, ip) } {
                    Some(bits) => bits,
                    None => return -1,
                },
                None => return -1,
            }
        };
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return -1;
        }
        let n = unsafe { list_len(ptr) } as isize;
        let low = ilow.clamp(0, n) as usize;
        let high = ihigh.clamp(low as isize, n) as usize;
        let exact_projection = if future_pointers.is_null() {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts(future_pointers, future_len) })
        };
        c_int::from(!unsafe {
            crate::object::list_mutation::replace_range_with_projection(
                &_py,
                ptr,
                low,
                high,
                &replacement,
                exact_projection,
            )
        })
    })
}

unsafe extern "C" fn hook_alloc_tuple(n: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_tuple_uninitialized(&_py, n);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_tuple_set(
    bits: u64,
    i: usize,
    val_bits: u64,
    _exact_pointer: *mut PyObject,
) -> OwnedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return OwnedHandleResult::error(),
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return OwnedHandleResult::error();
    }
    with_gil(|_py| {
        match unsafe { crate::object::seq_access::replace_unique_item(&_py, ptr, i, val_bits) } {
            Some(0) => OwnedHandleResult::missing(),
            Some(old_bits) => OwnedHandleResult::ok(old_bits),
            None => OwnedHandleResult::error(),
        }
    })
}

unsafe extern "C" fn hook_tuple_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return 0;
    }
    unsafe { tuple_len(ptr) }
}

unsafe extern "C" fn hook_tuple_item(bits: u64, i: usize) -> BorrowedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return BorrowedHandleResult::missing(),
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return BorrowedHandleResult::missing();
    }
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.get(i).copied())
    }
    .flatten()
    .filter(|item_bits| *item_bits != 0)
    .map(BorrowedHandleResult::ok)
    .unwrap_or_else(BorrowedHandleResult::missing)
}

unsafe extern "C" fn hook_alloc_dict() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_dict_with_pairs(&_py, &[]);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> i32 {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return -1;
        }
        unsafe { dict_set_in_place(&_py, ptr, key_bits, val_bits) };
        if crate::exception_pending(&_py) {
            -1
        } else {
            0
        }
    })
}

unsafe extern "C" fn hook_dict_get(dict_bits: u64, key_bits: u64) -> BorrowedHandleResult {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return BorrowedHandleResult::missing();
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return BorrowedHandleResult::missing();
        }
        match unsafe { dict_get_in_place(&_py, ptr, key_bits) } {
            Some(bits) => BorrowedHandleResult::ok(bits),
            None if crate::exception_pending(&_py) => BorrowedHandleResult::error(),
            None => BorrowedHandleResult::missing(),
        }
    })
}

unsafe extern "C" fn hook_dict_del(dict_bits: u64, key_bits: u64) -> i32 {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let ptr = match obj.as_ptr() {
            Some(p) => p,
            None => return -1,
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return -1;
        }
        if unsafe { dict_del_in_place(&_py, ptr, key_bits) } {
            0
        } else {
            -1
        }
    })
}

unsafe extern "C" fn hook_dict_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    unsafe { dict_len(ptr) }
}

/// Allocation-free O(1) dict cursor backing `PyDict_Next`. Reads the entry at
/// insertion-order `index` from the flat `[k0,v0,k1,v1,...]` order vector,
/// writing borrowed key/value bits into `*out_key`/`*out_val`. Returns 1 when an
/// entry exists at `index`, 0 at end-of-dict or on a non-dict argument. Mirrors
/// CPython's `PyDict_Next` ppos index into `dk_entries` and sets no exception.
unsafe extern "C" fn hook_dict_entry(
    dict_bits: u64,
    index: usize,
    out_key: *mut u64,
    out_val: *mut u64,
) -> i32 {
    let obj = MoltObject::from_bits(dict_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    let order = unsafe { dict_order(ptr) };
    let base = index.checked_mul(2);
    match base {
        Some(b) if b + 1 < order.len() => {
            if !out_key.is_null() {
                unsafe { *out_key = order[b] };
            }
            if !out_val.is_null() {
                unsafe { *out_val = order[b + 1] };
            }
            1
        }
        _ => 0,
    }
}

unsafe extern "C" fn hook_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_STRING {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { string_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { string_bytes(ptr) }
        }
    }
}

unsafe extern "C" fn hook_bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { bytes_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { bytes_data(ptr) }
        }
    }
}

unsafe extern "C" fn hook_buffer_acquire(bits: u64, out_view: *mut AbiMoltBufferView) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    let mut view = crate::MoltBufferView::default();
    let rc = unsafe { crate::c_api::molt_buffer_acquire(bits, &mut view as *mut _) };
    if rc != 0 {
        return rc;
    }
    unsafe {
        *out_view = abi_buffer_view_from_runtime(view);
    }
    0
}

unsafe extern "C" fn hook_buffer_release(view: *mut AbiMoltBufferView) -> i32 {
    if view.is_null() {
        return -1;
    }
    let mut runtime_view = unsafe { runtime_buffer_view_from_abi(*view) };
    let rc = unsafe { crate::c_api::molt_buffer_release(&mut runtime_view as *mut _) };
    unsafe {
        *view = AbiMoltBufferView::default();
    }
    rc
}

unsafe extern "C" fn hook_object_get_attr(obj_bits: u64, name_bits: u64) -> OwnedHandleResult {
    let bits = crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits);
    owned_result_from_pending(bits)
}

unsafe extern "C" fn hook_object_set_attr(obj_bits: u64, name_bits: u64, value_bits: u64) -> i32 {
    match crate::builtins::attributes::molt_set_attr_name(obj_bits, name_bits, value_bits) {
        0 => 0,
        _ => -1,
    }
}

/// `PyObject_Call` authority for bridge-managed Molt callables.
///
/// Routes through the runtime's single call authority (`molt_call_bind`):
/// compiled functions, types, bound methods, kwargs binding, and CPython-shaped
/// exceptions all live there. `args_bits` is a Molt tuple handle of positional
/// arguments (0 = none); `kwargs_bits` is a Molt dict handle (0 = none).
/// Returns result handle bits, or 0 with the error left in the runtime
/// pending-exception state (the ABI wrapper turns 0 into NULL-with-exception).
unsafe extern "C" fn hook_object_call(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> OwnedHandleResult {
    if args_bits == 0 {
        return unsafe { hook_object_call_with_pos(callable_bits, &[], kwargs_bits) };
    }
    let obj = MoltObject::from_bits(args_bits);
    let Some(ptr) = obj.as_ptr() else {
        return object_call_type_error("PyObject_Call args must be a tuple");
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return object_call_type_error("PyObject_Call args must be a tuple");
    }
    // The immutable tuple keeps every borrowed positional handle alive while
    // the scoped reader transfers it into call-argument custody. The slice
    // cannot escape even though binding may invoke Python.
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |pos| {
            hook_object_call_with_pos(callable_bits, pos, kwargs_bits)
        })
    }
    .unwrap_or_else(|| object_call_type_error("PyObject_Call args must be a tuple"))
}

unsafe fn hook_object_call_with_pos(
    callable_bits: u64,
    pos: &[u64],
    kwargs_bits: u64,
) -> OwnedHandleResult {
    // The ingress tuple/dict stay borrowed and live for the duration of this
    // hook. Scoped tuple access avoids the two temporary hot-path allocations.
    let (kw_order_ptr, kw_order_len) = if kwargs_bits != 0 {
        let obj = MoltObject::from_bits(kwargs_bits);
        let Some(ptr) = obj.as_ptr() else {
            return object_call_type_error("PyObject_Call kwargs must be a dict");
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return object_call_type_error("PyObject_Call kwargs must be a dict");
        }
        let order = unsafe { dict_order(ptr) };
        if order.len() % 2 != 0 {
            return object_call_type_error("PyObject_Call kwargs dict has malformed order storage");
        }
        (order.as_ptr(), order.len())
    } else {
        (std::ptr::null(), 0)
    };
    let builder_bits = crate::molt_callargs_new(pos.len() as u64, (kw_order_len / 2) as u64);
    if builder_bits == 0 {
        return OwnedHandleResult::error();
    }
    let release_builder = || {
        with_gil(|_py| crate::dec_ref_bits(&_py, builder_bits));
    };
    for &arg in pos {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if with_gil(|_py| crate::exception_pending(&_py)) {
            release_builder();
            return OwnedHandleResult::error();
        }
    }
    let kw_order = if kw_order_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(kw_order_ptr, kw_order_len) }
    };
    for pair in kw_order.chunks_exact(2) {
        let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, pair[0], pair[1]) };
        if with_gil(|_py| crate::exception_pending(&_py)) {
            release_builder();
            return OwnedHandleResult::error();
        }
    }
    // `molt_call_bind` takes builder custody immediately via PtrDropGuard and
    // destroys it on every return. Only failures before this call use the
    // explicit release closure above.
    let result = crate::molt_call_bind(callable_bits, builder_bits);
    if with_gil(|_py| crate::exception_pending(&_py)) {
        with_gil(|_py| crate::dec_ref_bits(&_py, result));
        return OwnedHandleResult::error();
    }
    if result == 0 {
        with_gil(|_py| {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "runtime call authority returned reserved zero without an exception",
            );
        });
        return OwnedHandleResult::error();
    }
    OwnedHandleResult::ok(result)
}

/// Allocate a `TYPE_ID_FOREIGN` wrapper around a genuine C-extension `PyObject*`
/// crossing INTO compiled Python. The bridge caller takes the strong reference
/// custody; this hook only materializes the Molt heap wrapper.
unsafe extern "C" fn hook_foreign_new(c_ptr: usize) -> u64 {
    with_gil(|_py| crate::object::foreign::foreign_new(&_py, c_ptr))
}

/// Raise a `TypeError` for a malformed `hook_object_call` argument shape and
/// return the hook's error sentinel (0).
fn object_call_type_error(message: &str) -> OwnedHandleResult {
    with_gil(|_py| {
        let _ = crate::raise_exception::<u64>(&_py, "TypeError", message);
    });
    OwnedHandleResult::error()
}

unsafe extern "C" fn hook_object_format(obj_bits: u64, spec_bits: u64) -> OwnedHandleResult {
    let bits = crate::molt_format_builtin(obj_bits, spec_bits);
    owned_result_from_pending(bits)
}

/// Route the ABI's `repr(float)` / `str(float)` through the runtime's single
/// float-format authority (`crate::object::float_repr::repr_float`), so the
/// C-API path produces byte-identical output to native `repr(float)`.
unsafe extern "C" fn hook_float_repr(value: f64, out: *mut u8, cap: usize) -> usize {
    let s = crate::object::float_repr::repr_float(value);
    let bytes = s.as_bytes();
    if bytes.len() <= cap && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

fn clear_speculative_sys_lookup_exception(had_pending_exception: bool) {
    if !had_pending_exception && with_gil(|_py| crate::exception_pending(&_py)) {
        let _ = crate::molt_exception_clear();
    }
}

/// Resolve a borrowed attribute of the compiled `sys` module through import custody.
///
/// The Python-side `sys` module owns the CPython-shaped views (e.g.
/// `sys.flags` is an attribute-carrying tuple subclass built from the raw
/// payload dict), so ABI consumers read the module attribute rather than a
/// duplicate raw-intrinsic lane. Returns 0 when the compiled program does not
/// link the `sys` module or the attribute is absent.
fn sys_module_attr_borrowed(attr: &[u8]) -> BorrowedHandleResult {
    let had_pending_exception = with_gil(|_py| crate::exception_pending(&_py));
    // Cache-first: PySys_GetObject is called from extension init code that
    // may already be inside the import machinery; re-entering a full import
    // for "sys" from there can deadlock on the import transaction and pays
    // the dispatcher on every call. The module cache is the custody for
    // already-initialized modules; the full import stays as the cold
    // bootstrap fallback.
    let name_bits = with_gil(|_py| {
        let name_ptr = alloc_string(&_py, b"sys");
        if name_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(name_ptr).bits()
    });
    let mut module_bits = 0u64;
    if MoltObject::from_bits(name_bits).as_ptr().is_some() {
        module_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
        with_gil(|_py| dec_ref_bits(&_py, name_bits));
        if MoltObject::from_bits(module_bits).as_ptr().is_none() {
            clear_speculative_sys_lookup_exception(had_pending_exception);
            module_bits = 0;
        }
    }
    if module_bits == 0 {
        module_bits = unsafe { hook_import_module(b"sys".as_ptr(), 3) };
    }
    if MoltObject::from_bits(module_bits).as_ptr().is_none() {
        clear_speculative_sys_lookup_exception(had_pending_exception);
        return BorrowedHandleResult::missing();
    }
    let out = with_gil(|_py| unsafe {
        let module_obj = MoltObject::from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return 0;
        };
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return 0;
        }
        let dict_bits = module_dict_bits(module_ptr);
        let Some(dict_ptr) = MoltObject::from_bits(dict_bits).as_ptr() else {
            return 0;
        };
        if let Some(bits) = dict_get_str_bytes_borrowed(&_py, dict_ptr, attr) {
            return bits;
        }
        // The compiled `sys` module materializes its CPython-shaped metadata
        // views (flags, implementation, version_info, float_info, hash_info, …)
        // lazily through its PEP 562 module `__getattr__`: the raw module dict
        // stays cold until Python code first touches the attribute. C
        // extensions read these through `PySys_GetObject`, which lands here
        // BEFORE any Python code runs — e.g. numpy's `_multiarray_umath`
        // multi-phase init reads `sys.flags` from its `Py_mod_exec` slot, and a
        // cold miss surfaces as "cannot get sys.flags" and fails the whole
        // C-extension import closed. Drive the module `__getattr__` (which
        // populates the module dict as a side effect), then re-read the
        // borrowed handle the dict now owns so the returned reference keeps the
        // CPython borrowed-reference contract of `PySys_GetObject`.
        let name_ptr = alloc_string(&_py, attr);
        if name_ptr.is_null() {
            return 0;
        }
        let attr_bits = MoltObject::from_ptr(name_ptr).bits();
        let materialized =
            crate::builtins::attr::module_attr_lookup_allow_missing(&_py, module_ptr, attr_bits);
        dec_ref_bits(&_py, attr_bits);
        if let Some(owned_bits) = materialized {
            // `__getattr__` stored the value into the module dict; drop the
            // owned reference it returned and hand back the borrowed handle the
            // dict keeps alive.
            dec_ref_bits(&_py, owned_bits);
        }
        dict_get_str_bytes_borrowed(&_py, dict_ptr, attr).unwrap_or(0)
    });
    with_gil(|_py| {
        dec_ref_bits(&_py, module_bits);
    });
    if out == 0 {
        clear_speculative_sys_lookup_exception(had_pending_exception);
    }
    if out == 0 {
        BorrowedHandleResult::missing()
    } else {
        BorrowedHandleResult::ok(out)
    }
}

unsafe extern "C" fn hook_sys_get_object_borrowed(
    name_data: *const u8,
    name_len: usize,
) -> BorrowedHandleResult {
    if name_data.is_null() {
        return BorrowedHandleResult::missing();
    }
    let name = match std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_data, name_len) })
    {
        Ok(name) => name,
        Err(_) => return BorrowedHandleResult::missing(),
    };
    sys_module_attr_borrowed(name.as_bytes())
}

unsafe extern "C" fn hook_eval_get_builtins_borrowed() -> BorrowedHandleResult {
    with_gil(|_py| {
        let as_builtins_dict = |bits: u64| -> Option<u64> {
            let ptr = crate::obj_from_bits(bits).as_ptr()?;
            match unsafe { object_type_id(ptr) } {
                TYPE_ID_DICT => Some(bits),
                TYPE_ID_MODULE => {
                    let dict_bits = unsafe { module_dict_bits(ptr) };
                    crate::obj_from_bits(dict_bits)
                        .as_ptr()
                        .is_some_and(|dict_ptr| unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT)
                        .then_some(dict_bits)
                }
                _ => None,
            }
        };
        let frame_builtins = crate::frame_stack_active_builtins_bits();
        if frame_builtins != 0 {
            return as_builtins_dict(frame_builtins).map_or_else(
                || {
                    let _ = crate::raise_exception::<u64>(
                        &_py,
                        "SystemError",
                        "active frame has an invalid builtins dictionary",
                    );
                    BorrowedHandleResult::error()
                },
                BorrowedHandleResult::ok,
            );
        }
        let builtins_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(&_py);
            cache.lock().unwrap().get("builtins").copied()
        };
        if let Some(dict_bits) = builtins_bits.and_then(as_builtins_dict) {
            BorrowedHandleResult::ok(dict_bits)
        } else {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "interpreter builtins dictionary is unavailable",
            );
            BorrowedHandleResult::error()
        }
    })
}

unsafe extern "C" fn hook_classify_heap(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return MoltTypeTag::Other as u8,
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_STRING => MoltTypeTag::Str as u8,
        TYPE_ID_BYTES => MoltTypeTag::Bytes as u8,
        TYPE_ID_BIGINT => MoltTypeTag::Int as u8,
        TYPE_ID_COMPLEX => MoltTypeTag::Complex as u8,
        TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => MoltTypeTag::List as u8,
        TYPE_ID_TUPLE => MoltTypeTag::Tuple as u8,
        TYPE_ID_DICT => MoltTypeTag::Dict as u8,
        TYPE_ID_SET => MoltTypeTag::Set as u8,
        TYPE_ID_FROZENSET => MoltTypeTag::FrozenSet as u8,
        crate::TYPE_ID_TYPE => MoltTypeTag::Type as u8,
        TYPE_ID_MODULE => MoltTypeTag::Module as u8,
        crate::TYPE_ID_EXCEPTION => MoltTypeTag::Exception as u8,
        crate::TYPE_ID_OBJECT
            if with_gil(|_py| {
                (unsafe { crate::object_class_bits(ptr) }) == crate::builtin_classes(&_py).traceback
            }) =>
        {
            MoltTypeTag::Traceback as u8
        }
        _ => MoltTypeTag::Other as u8,
    }
}

unsafe extern "C" fn hook_object_hash(bits: u64) -> i64 {
    with_gil(|_py| {
        let hash = crate::object::ops::hash_bits_signed(&_py, bits);
        if crate::exception_pending(&_py) {
            -1
        } else if hash == -1 {
            -2
        } else {
            hash
        }
    })
}

unsafe extern "C" fn hook_complex_parts(bits: u64, real: *mut f64, imag: *mut f64) -> c_int {
    if real.is_null() || imag.is_null() {
        return -1;
    }
    let Some(ptr) = crate::builtins::numbers::complex_ptr_from_bits(bits) else {
        return -1;
    };
    let value = unsafe { *crate::builtins::numbers::complex_ref(ptr) };
    unsafe {
        *real = value.re;
        *imag = value.im;
    }
    0
}

unsafe extern "C" fn hook_complex_from_doubles(real: f64, imag: f64) -> OwnedHandleResult {
    let bits = with_gil(|_py| crate::builtins::numbers::complex_bits(&_py, real, imag));
    owned_result_from_pending(bits)
}

unsafe extern "C" fn hook_inc_ref(bits: u64) {
    with_gil(|_py| inc_ref_bits(&_py, bits));
}

unsafe extern "C" fn hook_dec_ref(bits: u64) {
    with_gil(|_py| dec_ref_bits(&_py, bits));
}

unsafe extern "C" fn hook_ref_count(bits: u64) -> usize {
    MoltObject::from_bits(bits).as_ptr().map_or(0, |ptr| {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).ref_count.load(Ordering::Acquire) as usize }
    })
}

unsafe extern "C" fn hook_try_mark_abi_view(bits: u64, present: c_int) -> c_int {
    crate::gil_assert();
    let Some(ptr) = MoltObject::from_bits(bits).as_ptr() else {
        return 1;
    };
    let type_id = unsafe { object_type_id(ptr) };
    if present != 0 && matches!(type_id, TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL) {
        // A published PyListObject requires one stable generic storage
        // authority. Compact int/bool representations cannot retain an ABI
        // view because later promotion would otherwise replace their backing
        // allocation behind C's ob_item pointer.
        with_gil(|_py| unsafe {
            crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr)
        });
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return 0;
        }
        unsafe { crate::object::gc::gc_track_if_cyclic(ptr, TYPE_ID_LIST) };
    }
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if present != 0 {
            if ((*header).flags & crate::object::HEADER_FLAG_DEALLOCATING) != 0 {
                return 0;
            }
            (*header).flags |= crate::object::HEADER_FLAG_HAS_ABI_VIEW;
        } else {
            (*header).flags &= !crate::object::HEADER_FLAG_HAS_ABI_VIEW;
        }
    }
    1
}

// ─── Module / C-extension support ────────────────────────────────────────

unsafe extern "C" fn hook_alloc_module(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let name_ptr = alloc_string(&_py, bytes);
        if name_ptr.is_null() {
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_ptr = alloc_module_obj(&_py, name_bits);
        // alloc_module_obj inc_ref's the name; drop the local reference.
        dec_ref_bits(&_py, name_bits);
        if module_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(module_ptr).bits()
    })
}

unsafe extern "C" fn hook_import_module(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() || name_len == 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    let name_bits = with_gil(|_py| {
        let name_ptr = alloc_string(&_py, bytes);
        if name_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(name_ptr).bits()
    });
    if MoltObject::from_bits(name_bits).as_ptr().is_none() {
        return 0;
    }
    // molt_module_import owns its own GIL entry and returns an owned module
    // reference; import failures stay in the runtime pending-exception state
    // so the ABI-side module-init diagnostics can drain the real error.
    let module_bits = crate::builtins::modules::molt_module_import(name_bits);
    with_gil(|_py| dec_ref_bits(&_py, name_bits));
    match MoltObject::from_bits(module_bits).as_ptr() {
        Some(_) => module_bits,
        None => 0,
    }
}

unsafe extern "C" fn hook_exception_pending() -> std::os::raw::c_int {
    with_gil(|_py| crate::exception_pending(&_py) as std::os::raw::c_int)
}

unsafe extern "C" fn hook_report_unraisable(
    context_bits: u64,
    type_bits: u64,
    value_bits: u64,
    traceback_bits: u64,
    message: *const u8,
    message_len: usize,
    err_msg: *const u8,
    err_msg_len: usize,
    has_err_msg: c_int,
) {
    let message = if message.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(message, message_len) }
    };
    let err_msg = if has_err_msg != 0 {
        if err_msg.is_null() {
            Some("")
        } else {
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(err_msg, err_msg_len) }).ok()
        }
    } else {
        None
    };
    let fallback = || {
        let text = std::str::from_utf8(message).unwrap_or("<non-UTF-8 C API exception>");
        eprintln!(
            "Exception ignored in C API callback (type=0x{type_bits:x}, context=0x{context_bits:x}): {text}"
        );
    };
    with_gil(|_py| {
        let owned_message_bits = if crate::obj_from_bits(value_bits).is_none() {
            let msg_ptr = crate::alloc_string(&_py, message);
            if msg_ptr.is_null() {
                fallback();
                return;
            }
            Some(MoltObject::from_ptr(msg_ptr).bits())
        } else {
            None
        };
        let payload_bits = owned_message_bits.unwrap_or(value_bits);
        if let Some(value_ptr) = crate::obj_from_bits(value_bits).as_ptr()
            && unsafe { crate::object_type_id(value_ptr) } == crate::TYPE_ID_EXCEPTION
        {
            if !crate::obj_from_bits(traceback_bits).is_none() {
                let attached = crate::builtins::exceptions::molt_exception_with_traceback(
                    value_bits,
                    traceback_bits,
                );
                if !crate::obj_from_bits(attached).is_none() {
                    crate::dec_ref_bits(&_py, attached);
                }
            }
            crate::builtins::exceptions::report_captured_unraisable(
                &_py,
                context_bits,
                value_bits,
                err_msg,
            );
            return;
        }
        let args_ptr = crate::alloc_tuple(&_py, &[payload_bits]);
        if args_ptr.is_null() {
            if let Some(bits) = owned_message_bits {
                crate::dec_ref_bits(&_py, bits);
            }
            fallback();
            return;
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = if crate::obj_from_bits(type_bits).as_ptr().is_some() {
            type_bits
        } else {
            crate::exception_type_bits_from_name(&_py, "RuntimeError")
        };
        let exc_ptr = crate::alloc_exception_from_class_bits(&_py, class_bits, args_bits);
        crate::dec_ref_bits(&_py, args_bits);
        if let Some(bits) = owned_message_bits {
            crate::dec_ref_bits(&_py, bits);
        }
        if exc_ptr.is_null() {
            fallback();
            return;
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        if !crate::obj_from_bits(traceback_bits).is_none() {
            let attached = crate::builtins::exceptions::molt_exception_with_traceback(
                exc_bits,
                traceback_bits,
            );
            if !crate::obj_from_bits(attached).is_none() {
                crate::dec_ref_bits(&_py, attached);
            }
        }
        crate::builtins::exceptions::report_captured_unraisable(
            &_py,
            context_bits,
            exc_bits,
            err_msg,
        );
        crate::dec_ref_bits(&_py, exc_bits);
    });
}

// ── Numeric protocol (PyNumber_*) ─────────────────────────────────────────
//
// The single numeric authority is the runtime's `PyNumber_*` compat functions
// (`crate::c_api::PyNumber_*`), which delegate to `molt_add`/`molt_pow`/etc.
// with arbitrary-precision int promotion, float coercion, operator-overload
// dispatch, and CPython-shaped exceptions. Each returns result handle bits or
// `0` with a pending runtime exception on error. These hooks are a thin routing
// layer; they perform NO arithmetic themselves.

fn runtime_exception_field(field: u32) -> Option<crate::builtins::exceptions::ExceptionFieldSlot> {
    use crate::builtins::exceptions::ExceptionFieldSlot;
    use molt_cpython_abi::ExceptionField;
    match field {
        value if value == ExceptionField::Cause as u32 => Some(ExceptionFieldSlot::Cause),
        value if value == ExceptionField::Context as u32 => Some(ExceptionFieldSlot::Context),
        value if value == ExceptionField::Traceback as u32 => Some(ExceptionFieldSlot::Traceback),
        value if value == ExceptionField::Args as u32 => Some(ExceptionFieldSlot::Args),
        _ => None,
    }
}

/// Canonical exception normalization for the C error indicator. The ABI has
/// already shaped `_PyErr_CreateException`'s args tuple; invoke the requested
/// managed class through the same call/bind authority as ordinary Python so
/// custom `__new__`/`__init__` and constructor validation remain authoritative.
unsafe extern "C" fn hook_normalize_exception(
    requested_class_bits: u64,
    args_bits: u64,
    value_bits: u64,
    has_value: c_int,
    traceback_bits: u64,
    has_traceback: c_int,
    out_actual_class_bits: *mut u64,
) -> OwnedHandleResult {
    if out_actual_class_bits.is_null() {
        return OwnedHandleResult::error();
    }
    let valid_class = with_gil(|_py| {
        let Some(class_ptr) = crate::obj_from_bits(requested_class_bits).as_ptr() else {
            return false;
        };
        (unsafe { crate::object_type_id(class_ptr) }) == crate::TYPE_ID_TYPE
            && crate::issubclass_bits(
                requested_class_bits,
                crate::builtin_classes(&_py).base_exception,
            )
    });
    if !valid_class {
        with_gil(|_py| {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        });
        return OwnedHandleResult::error();
    }
    let existing_exception_bits = with_gil(|_py| {
        if has_value == 0 {
            return None;
        }
        let value_ptr = crate::obj_from_bits(value_bits).as_ptr()?;
        if unsafe { crate::object_type_id(value_ptr) } != crate::TYPE_ID_EXCEPTION {
            return None;
        }
        let actual_class_bits = unsafe { crate::object_class_bits(value_ptr) };
        crate::issubclass_bits(actual_class_bits, requested_class_bits).then_some(value_bits)
    });
    let exception_bits = if let Some(existing_bits) = existing_exception_bits {
        with_gil(|_py| crate::inc_ref_bits(&_py, existing_bits));
        existing_bits
    } else {
        let result = unsafe { hook_object_call(requested_class_bits, args_bits, 0) };
        let molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) = result.decode() else {
            return OwnedHandleResult::error();
        };
        bits
    };
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return OwnedHandleResult::error();
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            crate::dec_ref_bits(&_py, exception_bits);
            let _ = crate::raise_exception::<u64>(
                &_py,
                "TypeError",
                "calling an exception class did not return a BaseException instance",
            );
            return OwnedHandleResult::error();
        }
        if has_traceback != 0
            && crate::builtins::exceptions::exception_replace_field_bits(
                &_py,
                exception_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Traceback,
                traceback_bits,
            )
            .is_err()
        {
            crate::dec_ref_bits(&_py, exception_bits);
            return OwnedHandleResult::error();
        }
        // `_PyErr_SetObject` implicitly chains from the interpreter's active
        // handled exception, not from a parallel ABI-local guess. Preserve a
        // context explicitly supplied by a custom constructor and avoid a
        // self-cycle when an existing matching instance is restored.
        if crate::obj_from_bits(unsafe { crate::exception_context_bits(exception_ptr) }).is_none()
            && let Some(context_bits) = crate::builtins::exceptions::exception_context_active_bits()
            && context_bits != exception_bits
            && crate::builtins::exceptions::exception_replace_field_bits(
                &_py,
                exception_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                context_bits,
            )
            .is_err()
        {
            crate::dec_ref_bits(&_py, exception_bits);
            return OwnedHandleResult::error();
        }
        let actual_class_bits = unsafe { crate::object_class_bits(exception_ptr) };
        if actual_class_bits == 0 {
            crate::dec_ref_bits(&_py, exception_bits);
            return OwnedHandleResult::error();
        }
        unsafe {
            *out_actual_class_bits = actual_class_bits;
        }
        OwnedHandleResult::ok(exception_bits)
    })
}

unsafe extern "C" fn hook_exception_set_field(
    exception_bits: u64,
    field: u32,
    value_bits: u64,
    has_value: c_int,
) -> c_int {
    with_gil(|_py| {
        let Some(field) = runtime_exception_field(field) else {
            return -1;
        };
        let value_bits = if has_value == 0 {
            MoltObject::none().bits()
        } else {
            value_bits
        };
        crate::builtins::exceptions::exception_replace_field_bits(
            &_py,
            exception_bits,
            field,
            value_bits,
        )
        .map_or(-1, |()| 0)
    })
}

unsafe extern "C" fn hook_exception_get_field(
    exception_bits: u64,
    field: u32,
) -> OwnedHandleResult {
    with_gil(|_py| {
        let Some(field) = runtime_exception_field(field) else {
            return OwnedHandleResult::error();
        };
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return OwnedHandleResult::error();
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return OwnedHandleResult::error();
        }
        let value_bits = match field {
            crate::builtins::exceptions::ExceptionFieldSlot::Cause => unsafe {
                crate::exception_cause_bits(exception_ptr)
            },
            crate::builtins::exceptions::ExceptionFieldSlot::Context => unsafe {
                crate::exception_context_bits(exception_ptr)
            },
            crate::builtins::exceptions::ExceptionFieldSlot::Traceback => {
                crate::exception_materialize_traceback_bits(&_py, exception_ptr)
            }
            crate::builtins::exceptions::ExceptionFieldSlot::Args => {
                crate::exception_materialized_args_bits(&_py, exception_ptr)
            }
            crate::builtins::exceptions::ExceptionFieldSlot::Dict => unsafe {
                crate::exception_dict_bits(exception_ptr)
            },
            crate::builtins::exceptions::ExceptionFieldSlot::Notes => unsafe {
                crate::exception_notes_bits(exception_ptr)
            },
        };
        if crate::exception_pending(&_py) {
            return OwnedHandleResult::error();
        }
        if !matches!(field, crate::builtins::exceptions::ExceptionFieldSlot::Args)
            && crate::obj_from_bits(value_bits).is_none()
        {
            return OwnedHandleResult::missing();
        }
        if crate::obj_from_bits(value_bits).is_none() {
            return OwnedHandleResult::error();
        }
        crate::inc_ref_bits(&_py, value_bits);
        OwnedHandleResult::ok(value_bits)
    })
}

unsafe extern "C" fn hook_exception_class_borrowed(exception_bits: u64) -> BorrowedHandleResult {
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return BorrowedHandleResult::error();
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return BorrowedHandleResult::error();
        }
        let class_bits = unsafe { crate::object_class_bits(exception_ptr) };
        if class_bits == 0 {
            BorrowedHandleResult::error()
        } else {
            BorrowedHandleResult::ok(class_bits)
        }
    })
}

unsafe extern "C" fn hook_exception_snapshot(
    exception_bits: u64,
    out: *mut ExceptionSnapshot,
) -> c_int {
    if out.is_null() {
        return -1;
    }
    unsafe { out.write(ExceptionSnapshot::default()) };
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return -1;
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return -1;
        }
        let args = crate::exception_materialized_args_bits(&_py, exception_ptr);
        let traceback = crate::exception_materialize_traceback_bits(&_py, exception_ptr);
        if crate::exception_pending(&_py) || crate::obj_from_bits(args).is_none() {
            return -1;
        }
        let dict = unsafe { crate::exception_dict_bits(exception_ptr) };
        let notes = unsafe { crate::exception_notes_bits(exception_ptr) };
        let context = unsafe { crate::exception_context_bits(exception_ptr) };
        let cause = unsafe { crate::exception_cause_bits(exception_ptr) };
        let suppress =
            crate::obj_from_bits(unsafe { crate::exception_suppress_bits(exception_ptr) })
                .as_bool()
                .unwrap_or(false);
        let mut snapshot = ExceptionSnapshot {
            present_mask: EXCEPTION_SNAPSHOT_ARGS,
            suppress_context: u32::from(suppress),
            args,
            ..ExceptionSnapshot::default()
        };
        for (mask, bits, slot) in [
            (EXCEPTION_SNAPSHOT_DICT, dict, &raw mut snapshot.dict),
            (EXCEPTION_SNAPSHOT_NOTES, notes, &raw mut snapshot.notes),
            (
                EXCEPTION_SNAPSHOT_TRACEBACK,
                traceback,
                &raw mut snapshot.traceback,
            ),
            (
                EXCEPTION_SNAPSHOT_CONTEXT,
                context,
                &raw mut snapshot.context,
            ),
            (EXCEPTION_SNAPSHOT_CAUSE, cause, &raw mut snapshot.cause),
        ] {
            if !crate::obj_from_bits(bits).is_none() {
                snapshot.present_mask |= mask;
                unsafe { *slot = bits };
            }
        }
        for bits in [
            snapshot.dict,
            snapshot.args,
            snapshot.notes,
            snapshot.traceback,
            snapshot.context,
            snapshot.cause,
        ] {
            if bits != 0 {
                inc_ref_bits(&_py, bits);
            }
        }
        unsafe { out.write(snapshot) };
        0
    })
}

unsafe extern "C" fn hook_exception_commit_snapshot(
    exception_bits: u64,
    snapshot: *const ExceptionSnapshot,
) -> c_int {
    if snapshot.is_null() {
        return -1;
    }
    let snapshot = unsafe { *snapshot };
    let known_mask = EXCEPTION_SNAPSHOT_DICT
        | EXCEPTION_SNAPSHOT_ARGS
        | EXCEPTION_SNAPSHOT_NOTES
        | EXCEPTION_SNAPSHOT_TRACEBACK
        | EXCEPTION_SNAPSHOT_CONTEXT
        | EXCEPTION_SNAPSHOT_CAUSE;
    if snapshot.present_mask & !known_mask != 0
        || snapshot.present_mask & EXCEPTION_SNAPSHOT_ARGS == 0
        || snapshot.suppress_context > 1
    {
        return -1;
    }
    let field = |mask: u32, bits: u64| -> Option<u64> {
        if snapshot.present_mask & mask == 0 {
            (bits == 0).then_some(MoltObject::none().bits())
        } else {
            (bits != 0 && !crate::obj_from_bits(bits).is_none()).then_some(bits)
        }
    };
    let Some(dict) = field(EXCEPTION_SNAPSHOT_DICT, snapshot.dict) else {
        return -1;
    };
    let Some(args) = field(EXCEPTION_SNAPSHOT_ARGS, snapshot.args) else {
        return -1;
    };
    let Some(notes) = field(EXCEPTION_SNAPSHOT_NOTES, snapshot.notes) else {
        return -1;
    };
    let Some(traceback) = field(EXCEPTION_SNAPSHOT_TRACEBACK, snapshot.traceback) else {
        return -1;
    };
    let Some(context) = field(EXCEPTION_SNAPSHOT_CONTEXT, snapshot.context) else {
        return -1;
    };
    let Some(cause) = field(EXCEPTION_SNAPSHOT_CAUSE, snapshot.cause) else {
        return -1;
    };
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return -1;
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return -1;
        }
        let valid_args = crate::obj_from_bits(args)
            .as_ptr()
            .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) } == TYPE_ID_TUPLE);
        let valid_dict = crate::obj_from_bits(dict).is_none()
            || crate::obj_from_bits(dict)
                .as_ptr()
                .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) } == TYPE_ID_DICT);
        let valid_chain = |bits: u64| {
            crate::obj_from_bits(bits).is_none()
                || crate::obj_from_bits(bits)
                    .as_ptr()
                    .is_some_and(|ptr| unsafe {
                        crate::object_type_id(ptr) == crate::TYPE_ID_EXCEPTION
                    })
        };
        let valid_traceback = crate::obj_from_bits(traceback).is_none()
            || (crate::builtin_classes(&_py).traceback != 0
                && crate::isinstance_bits(&_py, traceback, crate::builtin_classes(&_py).traceback));
        if !valid_args
            || !valid_dict
            || !valid_chain(context)
            || !valid_chain(cause)
            || !valid_traceback
        {
            return -1;
        }
        // Nothing below this point can fail. Pin all new edges, publish every
        // public field as one GIL-serialized transaction, then release the old
        // graph. A rejected snapshot leaves the exception byte-for-byte intact.
        unsafe {
            crate::builtins::exceptions::exception_commit_snapshot_unchecked(
                &_py,
                exception_ptr,
                [dict, args, notes, traceback, context, cause],
                snapshot.suppress_context != 0,
            )
        };
        0
    })
}

unsafe extern "C" fn hook_type_is_subtype(subclass_bits: u64, class_bits: u64) -> c_int {
    with_gil(|_py| c_int::from(crate::issubclass_bits(subclass_bits, class_bits)))
}

unsafe extern "C" fn hook_take_pending_exception(
    actual_class_bits: *mut u64,
    traceback_bits: *mut u64,
) -> OwnedHandleResult {
    if actual_class_bits.is_null() || traceback_bits.is_null() {
        return OwnedHandleResult::error();
    }
    let exception_bits = crate::builtins::exceptions::molt_exception_last_pending();
    let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
        return OwnedHandleResult::error();
    };
    // Detach the original pending edge while lazily materializing traceback.
    // Any failure then leaves its new exact exception pending instead of being
    // mistaken for the original indicator or cleared at the end.
    let _ = crate::builtins::exceptions::molt_exception_clear();
    let captured = with_gil(|_py| {
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return None;
        }
        let class_bits = unsafe { crate::object_class_bits(exception_ptr) };
        if class_bits == 0 {
            return None;
        }
        let trace_bits = crate::exception_materialize_traceback_bits(&_py, exception_ptr);
        if crate::exception_pending(&_py) {
            return None;
        }
        Some((class_bits, trace_bits))
    });
    let Some((class_bits, trace_bits)) = captured else {
        with_gil(|_py| crate::dec_ref_bits(&_py, exception_bits));
        return OwnedHandleResult::error();
    };
    unsafe {
        *actual_class_bits = class_bits;
        *traceback_bits = if crate::obj_from_bits(trace_bits).is_none() {
            0
        } else {
            trace_bits
        };
    }
    OwnedHandleResult::ok(exception_bits)
}

unsafe extern "C" fn hook_handled_exception_get() -> OwnedHandleResult {
    with_gil(|_py| {
        let Some(bits) = crate::builtins::exceptions::exception_context_active_bits() else {
            return OwnedHandleResult::missing();
        };
        crate::inc_ref_bits(&_py, bits);
        OwnedHandleResult::ok(bits)
    })
}

unsafe extern "C" fn hook_handled_exception_set(owned_exception_bits: u64) -> c_int {
    with_gil(|_py| {
        if owned_exception_bits == 0 {
            crate::builtins::exceptions::exception_context_set_abi(&_py, MoltObject::none().bits());
            return 0;
        }
        // CPython deliberately accepts any PyObject here: this API directly
        // replaces the thread state's handled-value slot without validating
        // that the value is a BaseException instance.
        crate::builtins::exceptions::exception_context_set_abi(&_py, owned_exception_bits);
        crate::dec_ref_bits(&_py, owned_exception_bits);
        0
    })
}

/// Binary numeric op. `op` matches [`molt_cpython_abi::NumberBinaryOp`].
unsafe extern "C" fn hook_number_binary_op(op: u32, a_bits: u64, b_bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::NumberBinaryOp;
    let bits = match op {
        x if x == NumberBinaryOp::Add as u32 => crate::c_api::PyNumber_Add(a_bits, b_bits),
        x if x == NumberBinaryOp::Subtract as u32 => {
            crate::c_api::PyNumber_Subtract(a_bits, b_bits)
        }
        x if x == NumberBinaryOp::Multiply as u32 => {
            crate::c_api::PyNumber_Multiply(a_bits, b_bits)
        }
        x if x == NumberBinaryOp::TrueDivide as u32 => {
            crate::c_api::PyNumber_TrueDivide(a_bits, b_bits)
        }
        x if x == NumberBinaryOp::FloorDivide as u32 => {
            crate::c_api::PyNumber_FloorDivide(a_bits, b_bits)
        }
        x if x == NumberBinaryOp::Remainder as u32 => {
            crate::c_api::PyNumber_Remainder(a_bits, b_bits)
        }
        x if x == NumberBinaryOp::Lshift as u32 => crate::c_api::PyNumber_Lshift(a_bits, b_bits),
        x if x == NumberBinaryOp::Rshift as u32 => crate::c_api::PyNumber_Rshift(a_bits, b_bits),
        x if x == NumberBinaryOp::And as u32 => crate::c_api::PyNumber_And(a_bits, b_bits),
        x if x == NumberBinaryOp::Or as u32 => crate::c_api::PyNumber_Or(a_bits, b_bits),
        x if x == NumberBinaryOp::Xor as u32 => crate::c_api::PyNumber_Xor(a_bits, b_bits),
        x if x == NumberBinaryOp::MatrixMultiply as u32 => {
            crate::c_api::PyNumber_MatrixMultiply(a_bits, b_bits)
        }
        // An unknown op discriminant is a build-time contract break between the
        // ABI enum and this dispatch. Fail closed with a SystemError rather than
        // returning a fake success value.
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyNumber binary op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

/// Unary numeric op. `op` matches [`molt_cpython_abi::NumberUnaryOp`].
unsafe extern "C" fn hook_number_unary_op(op: u32, a_bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::NumberUnaryOp;
    let bits = match op {
        x if x == NumberUnaryOp::Negative as u32 => crate::c_api::PyNumber_Negative(a_bits),
        x if x == NumberUnaryOp::Positive as u32 => crate::c_api::PyNumber_Positive(a_bits),
        x if x == NumberUnaryOp::Absolute as u32 => crate::c_api::PyNumber_Absolute(a_bits),
        x if x == NumberUnaryOp::Invert as u32 => crate::c_api::PyNumber_Invert(a_bits),
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyNumber unary op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

/// Ternary power `pow(base, exp, modulus)`. `mod_bits == 0` means two-arg pow.
unsafe extern "C" fn hook_number_power(
    a_bits: u64,
    b_bits: u64,
    mod_bits: u64,
) -> OwnedHandleResult {
    let bits = crate::c_api::PyNumber_Power(a_bits, b_bits, mod_bits);
    owned_result_from_pending(bits)
}

/// Dict copy/keys/values. `op` matches [`molt_cpython_abi::DictOp`]. Routes to
/// the runtime dict authority; returns 0 with a pending exception on error.
unsafe extern "C" fn hook_dict_op(op: u32, dict_bits: u64) -> u64 {
    use molt_cpython_abi::DictOp;
    match op {
        x if x == DictOp::Copy as u32 => crate::c_api::PyDict_Copy(dict_bits),
        x if x == DictOp::Keys as u32 => crate::c_api::PyDict_Keys(dict_bits),
        x if x == DictOp::Values as u32 => crate::c_api::PyDict_Values(dict_bits),
        x if x == DictOp::Items as u32 => crate::c_api::PyDict_Items(dict_bits),
        x if x == DictOp::Clear as u32 => {
            let result = crate::molt_dict_clear(dict_bits);
            if with_gil(|_py| crate::exception_pending(&_py)) {
                0
            } else {
                result
            }
        }
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyDict op: unknown operation discriminant",
            )
        }),
    }
}

unsafe extern "C" fn hook_set_op(op: u32, set_bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::SetOp;
    let bits = match op {
        x if x == SetOp::FrozenNew as u32 => crate::c_api::PyFrozenSet_New(set_bits),
        x if x == SetOp::Pop as u32 => crate::c_api::PySet_Pop(set_bits),
        x if x == SetOp::Clear as u32 => {
            let rc = crate::c_api::PySet_Clear(set_bits);
            if rc == 0 {
                MoltObject::none().bits()
            } else {
                0
            }
        }
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PySet op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

// ── Set protocol (PySet_*) ────────────────────────────────────────────────
//
// The single set authority is the runtime's `PySet_*` compat functions
// (`crate::c_api::PySet_*`), which delegate to the runtime set object's hash
// table (`molt_set_*` / `set_del_in_place`) with dedup, hashed membership,
// frozenset immutability, and CPython-shaped exceptions (TypeError for
// unhashable keys, SystemError for non-sets). These hooks are a thin routing
// layer; they perform NO set logic themselves.

/// `PySet_New(iterable)` — 0 (empty) or a populated set. Returns handle bits, or
/// 0 with a pending exception on error.
unsafe extern "C" fn hook_set_new(iterable_bits: u64) -> u64 {
    crate::c_api::PySet_New(iterable_bits)
}

/// `PySet_Size(anyset)` — element count, or -1 with a pending exception.
unsafe extern "C" fn hook_set_size(set_bits: u64) -> c_int {
    // PySet_Size returns isize; the ABI hook narrows to c_int. A set never holds
    // more than isize::MAX elements, and the only out-of-band value is -1
    // (error), which round-trips through c_int unchanged.
    crate::c_api::PySet_Size(set_bits) as c_int
}

/// `PySet_Contains(anyset, key)` — 1 / 0 / -1.
unsafe extern "C" fn hook_set_contains(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Contains(set_bits, key_bits)
}

/// `PySet_Add(set, key)` — 0 on success, -1 on error.
unsafe extern "C" fn hook_set_add(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Add(set_bits, key_bits)
}

/// `PySet_Discard(set, key)` — 1 (removed) / 0 (absent) / -1 (error).
unsafe extern "C" fn hook_set_discard(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Discard(set_bits, key_bits)
}

/// `PyObject_Dir(o)` — return `dir(o)` as a list. Routes to the runtime dir
/// authority (`molt_object_dir_method`), which walks the MRO / `__dict__` /
/// `__dir__`. Returns 0 with a pending exception on error so the ABI side fails
/// closed instead of fabricating an empty list.
unsafe extern "C" fn hook_object_dir(obj_bits: u64) -> u64 {
    let result = crate::molt_object_dir_method(obj_bits);
    if with_gil(|_py| crate::exception_pending(&_py)) {
        return 0;
    }
    result
}

unsafe extern "C" fn hook_module_get_dict_borrowed(module_bits: u64) -> BorrowedHandleResult {
    with_gil(|_py| {
        let module_obj = MoltObject::from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            let _ = crate::raise_exception::<u64>(&_py, "SystemError", "expected module object");
            return BorrowedHandleResult::error();
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            let _ = crate::raise_exception::<u64>(&_py, "SystemError", "expected module object");
            return BorrowedHandleResult::error();
        }
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let valid_dict = crate::obj_from_bits(dict_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_DICT);
        if valid_dict {
            BorrowedHandleResult::ok(dict_bits)
        } else {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "module object has no dictionary",
            );
            BorrowedHandleResult::error()
        }
    })
}

unsafe extern "C" fn hook_import_add_module_borrowed(
    name_data: *const u8,
    name_len: usize,
) -> BorrowedHandleResult {
    if name_data.is_null() {
        return with_gil(|_py| {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyImport_AddModule requires a module name",
            );
            BorrowedHandleResult::error()
        });
    }
    let name = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(&_py);
            cache.lock().unwrap().get("sys").copied()
        };
        let Some(sys_bits) = sys_bits else {
            let _ =
                crate::raise_exception::<u64>(&_py, "SystemError", "sys.modules is unavailable");
            return BorrowedHandleResult::error();
        };
        let Some(modules_bits) = crate::builtins::modules::sys_modules_dict_bits(&_py, sys_bits)
        else {
            if !crate::exception_pending(&_py) {
                let _ = crate::raise_exception::<u64>(
                    &_py,
                    "SystemError",
                    "sys.modules is unavailable",
                );
            }
            return BorrowedHandleResult::error();
        };
        let Some(modules_ptr) = crate::obj_from_bits(modules_bits).as_ptr() else {
            crate::dec_ref_bits(&_py, modules_bits);
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "sys.modules is not a dictionary",
            );
            return BorrowedHandleResult::error();
        };
        let name_ptr = alloc_string(&_py, name);
        if name_ptr.is_null() {
            crate::dec_ref_bits(&_py, modules_bits);
            return BorrowedHandleResult::error();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let existing = unsafe { dict_get_in_place(&_py, modules_ptr, name_bits) };
        if let Some(existing_bits) = existing
            && crate::obj_from_bits(existing_bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_MODULE)
        {
            crate::dec_ref_bits(&_py, name_bits);
            crate::dec_ref_bits(&_py, modules_bits);
            return BorrowedHandleResult::ok(existing_bits);
        }
        let module_ptr = alloc_module_obj(&_py, name_bits);
        if module_ptr.is_null() {
            crate::dec_ref_bits(&_py, name_bits);
            crate::dec_ref_bits(&_py, modules_bits);
            return BorrowedHandleResult::error();
        }
        let module_bits = MoltObject::from_ptr(module_ptr).bits();
        // `dict_set_in_place` is transactional for the mapping: every
        // fallible hash/equality/rebuild/reserve step completes before it
        // changes order/value storage. Once mutation starts, capacity is
        // reserved and publication is infallible. Therefore a pending error
        // means the prior non-module value (or absence) is still exact; never
        // run a second fallible dict operation under that error indicator.
        unsafe { dict_set_in_place(&_py, modules_ptr, name_bits, module_bits) };
        if crate::exception_pending(&_py) {
            crate::dec_ref_bits(&_py, module_bits);
            crate::dec_ref_bits(&_py, name_bits);
            crate::dec_ref_bits(&_py, modules_bits);
            return BorrowedHandleResult::error();
        }
        // sys.modules owns the published edge. Drop constructor temporaries and
        // return the dictionary's borrowed handle.
        crate::dec_ref_bits(&_py, module_bits);
        crate::dec_ref_bits(&_py, name_bits);
        crate::dec_ref_bits(&_py, modules_bits);
        BorrowedHandleResult::ok(module_bits)
    })
}

unsafe extern "C" fn hook_module_set_attr(
    module_bits: u64,
    name_data: *const u8,
    name_len: usize,
    value_bits: u64,
) -> std::os::raw::c_int {
    if name_data.is_null() {
        return -1;
    }
    let module_obj = MoltObject::from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return -1;
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return -1;
    }
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let dict_obj = MoltObject::from_bits(dict_bits);
        let Some(dict_ptr) = dict_obj.as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return -1;
        }
        let name_str_ptr = alloc_string(&_py, name_bytes);
        if name_str_ptr.is_null() {
            return -1;
        }
        let name_str_bits = MoltObject::from_ptr(name_str_ptr).bits();
        unsafe { dict_set_in_place(&_py, dict_ptr, name_str_bits, value_bits) };
        // dict_set_in_place takes its own references on key+value.  Drop our
        // local key reference; the caller still owns the value.
        dec_ref_bits(&_py, name_str_bits);
        0
    })
}

// ── PyCFunction → Molt callable bridge ───────────────────────────────────
//
// CPython C extensions register functions as PyCFunction pointers with a
// METH_* flag bitmask describing the calling convention.  Molt's call
// dispatch uses fixed-arity native functions (TYPE_ID_FUNCTION) with a
// trampoline slot for variadic dispatch.
//
// To bridge the two we maintain a small process-wide registry mapping each
// registered C function to its (meth_addr, flags) tuple.  The registry key
// is stored as a NaN-boxed int in the Molt function's `closure` slot, and
// every C function shares a single trampoline that decodes the registry id
// and forks on the calling convention.
//
unsafe extern "C" fn hook_module_capi_register(
    module_bits: u64,
    module_def_ptr: usize,
    module_state_size: u64,
) -> i32 {
    crate::c_api::molt_module_capi_register(module_bits, module_def_ptr, module_state_size)
}

unsafe extern "C" fn hook_module_capi_get_state(module_bits: u64) -> *mut u8 {
    crate::c_api::molt_module_capi_get_state(module_bits)
}

unsafe extern "C" fn hook_module_state_add(module_bits: u64, module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_add(module_bits, module_def_ptr)
}

unsafe extern "C" fn hook_module_state_find(module_def_ptr: usize) -> BorrowedHandleResult {
    match crate::c_api::molt_module_state_find(module_def_ptr) {
        0 => BorrowedHandleResult::missing(),
        bits => BorrowedHandleResult::ok(bits),
    }
}

unsafe extern "C" fn hook_module_state_remove(module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_remove(module_def_ptr)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CExtDispatchKind {
    NoArgs,
    OneObject,
    VarArgs,
    VarArgsKeywords,
    FastCall,
    FastCallKeywords,
}

impl CExtDispatchKind {
    fn from_flags(flags: i32) -> Option<Self> {
        let conv_flags = flags & !(METH_CLASS | METH_STATIC | METH_COEXIST);
        if conv_flags & METH_METHOD != 0 {
            return None;
        }
        let fastcall = conv_flags & METH_FASTCALL != 0;
        let keywords = conv_flags & METH_KEYWORDS != 0;
        let varargs = conv_flags & METH_VARARGS != 0;
        if fastcall {
            let allowed = METH_FASTCALL | METH_KEYWORDS;
            if conv_flags & !allowed != 0 {
                return None;
            }
            return Some(if keywords {
                Self::FastCallKeywords
            } else {
                Self::FastCall
            });
        }
        if keywords {
            let allowed = METH_VARARGS | METH_KEYWORDS;
            if conv_flags & !allowed != 0 || !varargs {
                return None;
            }
            return Some(Self::VarArgsKeywords);
        }
        match conv_flags {
            METH_NOARGS => Some(Self::NoArgs),
            METH_O => Some(Self::OneObject),
            METH_VARARGS => Some(Self::VarArgs),
            _ => None,
        }
    }

    fn arity(self) -> u64 {
        match self {
            Self::NoArgs => 0,
            Self::OneObject => 1,
            Self::VarArgs | Self::VarArgsKeywords | Self::FastCall | Self::FastCallKeywords => 0,
        }
    }

    fn is_variadic(self) -> bool {
        !matches!(self, Self::NoArgs | Self::OneObject)
    }
}

#[derive(Clone, Copy)]
struct CExtCallable {
    meth_addr: usize,
    flags: i32,
    self_bits: u64,
    dispatch_kind: CExtDispatchKind,
}

// SAFETY: meth_addr is a `*const ()` we transmute back to the original
// PyCFunction signature inside the trampoline.  The pointer is guaranteed
// valid for the process lifetime by `loader::LOADED_EXTENSION_LIBRARIES`.
unsafe impl Send for CExtCallable {}
unsafe impl Sync for CExtCallable {}

fn cext_callable_registry() -> &'static Mutex<Vec<CExtCallable>> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Mutex<Vec<CExtCallable>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_prepare_static_extension() -> u64 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    MoltObject::from_bool(true).bits()
}

unsafe fn static_module_def_to_bits(def: *mut PyModuleDef) -> Result<Option<u64>, String> {
    if def.is_null() {
        return Ok(None);
    }
    let name = unsafe { (*def).m_name };
    if name.is_null() {
        return Ok(None);
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if name_bytes.is_empty() {
        return Ok(None);
    }
    let module_name = String::from_utf8_lossy(name_bytes).into_owned();
    let spec_obj = unsafe {
        static_module_spec_for_def_name(name_bytes).ok_or_else(|| {
            format!("{module_name}: static-link PyModuleDef ModuleSpec bridge failed")
        })?
    };
    let module_obj =
        unsafe { molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(def, spec_obj, 0) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec_obj) };
    if module_obj.is_null() {
        if transfer_pending_cpython_exception() || with_gil(|_py| crate::exception_pending(&_py)) {
            return Err(String::new());
        }
        return Err(format!(
            "{module_name}: static-link PyModuleDef creation failed"
        ));
    }
    if unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module_obj, def) } != 0 {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module_obj) };
        let _ = transfer_pending_cpython_exception();
        return Err(format!(
            "{module_name}: static-link PyModuleDef Py_mod_exec slot returned non-zero"
        ));
    }
    let Some(module_bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(module_obj)
        .map(|value| value.bits())
    else {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module_obj) };
        return Err(format!("{module_name}: module view is not runtime-managed"));
    };
    let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module_obj) };
        return Err(format!(
            "{module_name}: static-link PyModuleDef returned an invalid module handle"
        ));
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module_obj) };
        return Err(format!(
            "{module_name}: static-link PyModuleDef returned a non-module object"
        ));
    }
    // Convert the new C reference into the owned runtime edge returned to the
    // caller. The physical module view is released on every exit.
    with_gil(|_py| crate::inc_ref_bits(&_py, module_bits));
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module_obj) };
    Ok(Some(module_bits))
}

unsafe fn cext_bytes_from_raw<'a>(data: *const u8, len: u64) -> Result<&'a [u8], &'static str> {
    let len = usize::try_from(len).map_err(|_| "byte length does not fit in usize")?;
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err("byte pointer must not be NULL when length is non-zero");
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

unsafe fn cext_optional_bytes_from_raw<'a>(
    data: *const u8,
    len: u64,
) -> Result<Option<&'a [u8]>, &'static str> {
    if data.is_null() && len == 0 {
        return Ok(None);
    }
    Ok(Some(unsafe { cext_bytes_from_raw(data, len)? }))
}

unsafe fn cext_set_str_attr(
    obj_bits: u64,
    attr_name: &[u8],
    value_bytes: &[u8],
) -> Result<(), &'static str> {
    let value_bits = unsafe { hook_alloc_str(value_bytes.as_ptr(), value_bytes.len()) };
    if value_bits == 0 {
        return Err("failed to allocate C extension function metadata string");
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            obj_bits,
            attr_name.as_ptr(),
            attr_name.len() as u64,
            value_bits,
        )
    };
    unsafe { hook_dec_ref(value_bits) };
    if rc != 0 {
        return Err("failed to attach C extension function metadata");
    }
    Ok(())
}

unsafe fn cext_create_py_cfunction_bits(
    self_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<u64, &'static str> {
    if name_bytes.is_empty() {
        return Err("PyMethodDef name must not be empty");
    }
    if method_addr == 0 {
        return Err("PyMethodDef method pointer must not be NULL");
    }
    let flags = i32::try_from(method_flags).map_err(|_| "PyMethodDef flags do not fit in c_int")?;
    if CExtDispatchKind::from_flags(flags).is_none() {
        return Err("unsupported PyMethodDef flags for CPython ABI bridge");
    }
    let func_bits = unsafe {
        hook_register_c_function(
            method_addr as u64,
            flags,
            self_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
        )
    };
    if func_bits == 0 {
        return Err("failed to register PyMethodDef callback with CPython ABI bridge");
    }
    if let Some(doc_bytes) = doc_bytes
        && unsafe { cext_set_str_attr(func_bits, b"__doc__", doc_bytes) }.is_err()
    {
        unsafe { hook_dec_ref(func_bits) };
        return Err("failed to attach PyMethodDef __doc__");
    }
    Ok(func_bits)
}

unsafe fn cext_attach_module_name(func_bits: u64, module_bits: u64) -> Result<(), &'static str> {
    let module_name_attr = b"__name__";
    let module_name_bits = unsafe {
        crate::c_api::molt_object_getattr_bytes(
            module_bits,
            module_name_attr.as_ptr(),
            module_name_attr.len() as u64,
        )
    };
    if MoltObject::from_bits(module_name_bits).is_none() {
        let _ = crate::molt_exception_clear();
        return Ok(());
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            func_bits,
            b"__module__".as_ptr(),
            b"__module__".len() as u64,
            module_name_bits,
        )
    };
    unsafe { hook_dec_ref(module_name_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef __module__");
    }
    Ok(())
}

unsafe fn cext_add_py_cfunction_to_module(
    module_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<(), &'static str> {
    let func_bits = unsafe {
        cext_create_py_cfunction_bits(
            module_bits,
            name_bytes,
            method_addr,
            method_flags,
            doc_bytes,
        )?
    };
    if let Err(message) = unsafe { cext_attach_module_name(func_bits, module_bits) } {
        unsafe { hook_dec_ref(func_bits) };
        return Err(message);
    }
    let rc = unsafe {
        hook_module_set_attr(
            module_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
            func_bits,
        )
    };
    unsafe { hook_dec_ref(func_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef callback to module");
    }
    Ok(())
}

/// # Safety
/// `name_ptr` must point to `name_len` readable bytes; `doc_ptr` must be null or
/// point to `doc_len` readable bytes. `method_addr` must be the address of a
/// valid C callable whose calling convention matches `method_flags`, and
/// `self_bits` must be a valid Molt object handle (or 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_py_cfunction_create_bytes(
    self_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> u64 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_create_py_cfunction_bits(
                self_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(bits) => bits,
            Err(message) => crate::raise_exception::<u64>(&_py, "TypeError", message),
        }
    })
}

/// # Safety
/// `name_ptr` must point to `name_len` readable bytes; `doc_ptr` must be null or
/// point to `doc_len` readable bytes. `method_addr` must be the address of a
/// valid C callable whose calling convention matches `method_flags`, and
/// `module_bits` must be a valid Molt module handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_py_cfunction_bytes(
    module_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> i32 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_add_py_cfunction_to_module(
                module_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(()) => 0,
            Err(message) => crate::raise_exception::<i32>(&_py, "TypeError", message),
        }
    })
}

unsafe fn static_pyinit_registered_bridge_module_bits(
    result_pyobj: *mut PyObject,
) -> Result<Option<u64>, &'static str> {
    let Some(module_bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(result_pyobj)
        .map(molt_cpython_abi::bridge::MoltValueHandle::bits)
    else {
        return Ok(None);
    };
    let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
        return Err("static extension PyInit returned a non-module object");
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err("static extension PyInit returned a non-module object");
    }
    Ok(Some(module_bits))
}

unsafe fn static_pyinit_type_matches(
    result_pyobj: *mut PyObject,
    canonical: *mut PyTypeObject,
    type_name: &[u8],
) -> bool {
    if result_pyobj.is_null() {
        return false;
    }
    let actual = unsafe { (*result_pyobj).ob_type };
    if actual.is_null() {
        return false;
    }
    if std::ptr::eq(actual, canonical) {
        return true;
    }
    let actual_name = unsafe { (*actual).tp_name };
    if actual_name.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(actual_name).to_bytes() == type_name }
}

unsafe fn static_pyinit_is_module_def(result_pyobj: *mut PyObject) -> bool {
    unsafe { static_pyinit_type_matches(result_pyobj, &raw mut PyModuleDef_Type, b"moduledef") }
}

unsafe fn static_pyinit_is_bridge_module_object(result_pyobj: *mut PyObject) -> bool {
    unsafe { static_pyinit_type_matches(result_pyobj, &raw mut PyModule_Type, b"module") }
}

unsafe fn static_pyinit_has_module_def_shape(result_pyobj: *mut PyObject) -> bool {
    if result_pyobj.is_null() {
        return false;
    }
    let def = result_pyobj as *mut PyModuleDef;
    let base = unsafe { &(*def).m_base };
    if base.m_init.is_some() || base.m_index != 0 || !base.m_copy.is_null() {
        return false;
    }
    let name = unsafe { (*def).m_name };
    if name.is_null() {
        return false;
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if name_bytes.is_empty() {
        return false;
    }
    if unsafe { (*def).m_size } < -1 {
        return false;
    }
    name_bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.'))
}

unsafe fn static_module_spec_for_def_name(name_bytes: &[u8]) -> Option<*mut PyObject> {
    let spec_type_name = b"importlib.machinery.ModuleSpec";
    let spec_bits = unsafe { hook_alloc_module(spec_type_name.as_ptr(), spec_type_name.len()) };
    if spec_bits == 0 {
        return None;
    }
    let none_bits = MoltObject::none().bits();
    let initialized = unsafe { static_module_spec_set_str(spec_bits, b"name", name_bytes) }
        && unsafe { static_module_spec_set_bits(spec_bits, b"loader", none_bits) }
        && unsafe { static_module_spec_set_bits(spec_bits, b"origin", none_bits) }
        && unsafe { static_module_spec_set_str(spec_bits, b"parent", b"") }
        && unsafe {
            static_module_spec_set_bits(spec_bits, b"submodule_search_locations", none_bits)
        };
    if !initialized {
        unsafe { hook_dec_ref(spec_bits) };
        return None;
    }
    Some(unsafe { cext_owned_pyobject_from_bits(spec_bits) })
}

unsafe fn static_module_spec_set_str(spec_bits: u64, attr: &[u8], value: &[u8]) -> bool {
    let value_bits = unsafe { hook_alloc_str(value.as_ptr(), value.len()) };
    if value_bits == 0 {
        return false;
    }
    let out = unsafe { static_module_spec_set_bits(spec_bits, attr, value_bits) };
    unsafe { hook_dec_ref(value_bits) };
    out
}

unsafe fn static_module_spec_set_bits(spec_bits: u64, attr: &[u8], value_bits: u64) -> bool {
    unsafe { hook_module_set_attr(spec_bits, attr.as_ptr(), attr.len(), value_bits) == 0 }
}

fn transfer_pending_cpython_exception() -> bool {
    let Some(error) = molt_cpython_abi::api::errors::take_current_error() else {
        return false;
    };
    let value_bits = if error.value.is_null() {
        MoltObject::none().bits()
    } else {
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(error.value) }
            .unwrap_or_else(|| MoltObject::none().bits())
    };
    let traceback_bits = if error.traceback.is_null() {
        MoltObject::none().bits()
    } else {
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(error.traceback) }
            .unwrap_or_else(|| MoltObject::none().bits())
    };
    let transferred = with_gil(|_py| {
        if let Some(value_ptr) = crate::obj_from_bits(value_bits).as_ptr()
            && unsafe { crate::object_type_id(value_ptr) } == crate::TYPE_ID_EXCEPTION
        {
            if crate::builtins::exceptions::exception_replace_field_bits(
                &_py,
                value_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Traceback,
                traceback_bits,
            )
            .is_err()
            {
                dec_ref_bits(&_py, value_bits);
                if !crate::obj_from_bits(traceback_bits).is_none() {
                    dec_ref_bits(&_py, traceback_bits);
                }
                let _ = crate::raise_exception::<u64>(
                    &_py,
                    "SystemError",
                    "C extension supplied an invalid traceback for its pending exception",
                );
                return false;
            }
            // Record through the canonical exception transition so an
            // already-pending runtime error becomes implicit context instead
            // of being silently overwritten by the C indicator transfer.
            crate::record_exception(&_py, value_ptr);
            dec_ref_bits(&_py, value_bits);
            if !crate::obj_from_bits(traceback_bits).is_none() {
                dec_ref_bits(&_py, traceback_bits);
            }
            return true;
        }
        if !crate::obj_from_bits(value_bits).is_none() {
            dec_ref_bits(&_py, value_bits);
        }
        if !crate::obj_from_bits(traceback_bits).is_none() {
            dec_ref_bits(&_py, traceback_bits);
        }
        let _ = crate::raise_exception::<u64>(
            &_py,
            "SystemError",
            "C error indicator did not contain a normalized runtime exception instance",
        );
        false
    });
    transferred || with_gil(|_py| crate::exception_pending(&_py))
}

fn cpython_error_is_pending() -> bool {
    !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
        || with_gil(|_py| crate::exception_pending(&_py))
}

struct NativePendingSnapshot {
    c_error: Option<molt_cpython_abi::api::errors::OwnedCError>,
    runtime_error_bits: Option<u64>,
}

impl NativePendingSnapshot {
    fn has_error(&self) -> bool {
        self.c_error.is_some() || self.runtime_error_bits.is_some()
    }
}

fn take_native_pending_snapshot() -> NativePendingSnapshot {
    let c_error = molt_cpython_abi::api::errors::take_current_error();
    let runtime_error_bits = with_gil(|_py| {
        if !crate::exception_pending(&_py) {
            return None;
        }
        let bits = crate::exception_last_bits_noinc(&_py)?;
        crate::inc_ref_bits(&_py, bits);
        crate::clear_exception(&_py);
        Some(bits)
    });
    NativePendingSnapshot {
        c_error,
        runtime_error_bits,
    }
}

fn restore_native_pending_snapshot(snapshot: NativePendingSnapshot) {
    // Discard any destructor/conversion failure produced while the original
    // channels were detached, then restore the exact originals in their
    // respective authorities.
    drop(molt_cpython_abi::api::errors::take_current_error());
    with_gil(|_py| {
        crate::clear_exception(&_py);
        if let Some(bits) = snapshot.runtime_error_bits {
            if let Some(ptr) = crate::obj_from_bits(bits).as_ptr() {
                crate::record_exception(&_py, ptr);
            }
            crate::dec_ref_bits(&_py, bits);
        }
    });
    if let Some(error) = snapshot.c_error {
        molt_cpython_abi::api::errors::restore_current_error_exact(error);
    }
}

fn raise_native_result_with_error(message: &str) -> i64 {
    let _ = transfer_pending_cpython_exception();
    with_gil(|_py| crate::raise_exception::<i64>(&_py, "SystemError", message))
}

fn static_pyinit_failure(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    let _ = transfer_pending_cpython_exception();
    let prior_bits = crate::builtins::exceptions::molt_exception_last_pending();
    let Some(prior_ptr) = crate::obj_from_bits(prior_bits).as_ptr() else {
        return crate::raise_exception::<u64>(_py, "ImportError", message);
    };
    let detail = crate::format_exception_message(_py, prior_ptr);
    let combined = if detail.is_empty() || message.contains(&detail) {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };

    // Static-link import adaptation is an error boundary of its own. Preserve
    // the extension's exact pending exception as __context__, but publish one
    // contextual ImportError so the module/phase and the original C-API detail
    // cannot be split across parallel error channels.
    crate::clear_exception(_py);
    let wrapper_ptr = crate::builtins::exceptions::alloc_exception(_py, "ImportError", &combined);
    if wrapper_ptr.is_null() {
        crate::dec_ref_bits(_py, prior_bits);
        return MoltObject::none().bits();
    }
    let wrapper_bits = MoltObject::from_ptr(wrapper_ptr).bits();
    if crate::builtins::exceptions::exception_replace_field_bits(
        _py,
        wrapper_bits,
        crate::builtins::exceptions::ExceptionFieldSlot::Context,
        prior_bits,
    )
    .is_err()
    {
        crate::dec_ref_bits(_py, wrapper_bits);
        crate::dec_ref_bits(_py, prior_bits);
        return MoltObject::none().bits();
    }
    crate::builtins::exceptions::record_exception_owned(_py, wrapper_ptr);
    crate::dec_ref_bits(_py, prior_bits);
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_pyinit_module_to_bits(result_pyobj: u64) -> u64 {
    with_gil(|_py| {
        let has_error = cpython_error_is_pending();
        if result_pyobj == 0 {
            if has_error {
                return static_pyinit_failure(&_py, "static extension PyInit returned NULL");
            }
            return crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "static extension PyInit returned NULL without setting an exception",
            );
        }
        let result_ptr = result_pyobj as *mut PyObject;
        if unsafe { static_pyinit_is_module_def(result_ptr) } {
            if has_error {
                let def = result_ptr.cast::<PyModuleDef>();
                let name = unsafe { (*def).m_name };
                let invalid_definition =
                    name.is_null() || unsafe { CStr::from_ptr(name) }.to_bytes().is_empty();
                let context = if invalid_definition {
                    "static extension PyInit returned an invalid module definition"
                } else {
                    "static extension PyInit returned a module definition with an exception set"
                };
                return static_pyinit_failure(&_py, context);
            }
            match unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) } {
                Ok(Some(module_bits)) => return module_bits,
                Ok(None) => {}
                Err(message) => {
                    let message = if message.is_empty() {
                        "static extension module initialization failed"
                    } else {
                        message.as_str()
                    };
                    return static_pyinit_failure(&_py, message);
                }
            }
            return static_pyinit_failure(
                &_py,
                "static extension PyInit returned an invalid module definition",
            );
        }
        if has_error {
            let pending = take_native_pending_snapshot();
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result_ptr) };
            restore_native_pending_snapshot(pending);
            return static_pyinit_failure(
                &_py,
                "static extension PyInit returned a result with an exception set",
            );
        }
        match unsafe { static_pyinit_registered_bridge_module_bits(result_ptr) } {
            Ok(Some(module_bits)) => return module_bits,
            Ok(None) => {}
            Err(message) => {
                return static_pyinit_failure(&_py, message);
            }
        }
        if unsafe { static_pyinit_is_bridge_module_object(result_ptr) } {
            let module_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(result_ptr)
                .map(|value| value.bits())
                .unwrap_or(0);
            if let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() {
                unsafe {
                    if object_type_id(module_ptr) == TYPE_ID_MODULE {
                        return module_bits;
                    }
                }
            }
        }
        if !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
            || crate::exception_pending(&_py)
        {
            return static_pyinit_failure(
                &_py,
                "static extension PyInit returned an invalid module handle",
            );
        }
        if unsafe { static_pyinit_has_module_def_shape(result_ptr) } {
            match unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) } {
                Ok(Some(module_bits)) => return module_bits,
                Ok(None) => {}
                Err(message) => {
                    let message = if message.is_empty() {
                        "static extension module initialization failed"
                    } else {
                        message.as_str()
                    };
                    return static_pyinit_failure(&_py, message);
                }
            }
            return static_pyinit_failure(
                &_py,
                "static extension PyInit returned an invalid module definition",
            );
        }
        static_pyinit_failure(
            &_py,
            "static extension PyInit returned an invalid module handle",
        )
    })
}

unsafe fn cext_owned_pyobject_from_bits(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

unsafe fn cext_new_pyobject_from_borrowed_bits(bits: u64) -> *mut PyObject {
    let ptr = unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    if !ptr.is_null() {
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(ptr) };
    }
    ptr
}

unsafe fn cext_tuple_for_args(args: &[u64]) -> Option<*mut PyObject> {
    let tuple_bits = unsafe { hook_alloc_tuple(args.len()) };
    if tuple_bits == 0 {
        return None;
    }
    for (index, &arg_bits) in args.iter().enumerate() {
        match unsafe { hook_tuple_set(tuple_bits, index, arg_bits, ptr::null_mut()) }.decode() {
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(old_bits) => unsafe {
                hook_dec_ref(old_bits)
            },
            molt_cpython_abi::hooks::DecodedHandleResult::Missing => {}
            molt_cpython_abi::hooks::DecodedHandleResult::Error => {
                unsafe { hook_dec_ref(tuple_bits) };
                return None;
            }
        }
    }
    let tuple_obj = unsafe { cext_owned_pyobject_from_bits(tuple_bits) };
    if tuple_obj.is_null() {
        return None;
    }
    Some(tuple_obj)
}

struct CExtIngress {
    owned_views: Vec<*mut PyObject>,
}

impl CExtIngress {
    fn with_capacity(capacity: usize) -> Option<Self> {
        let mut owned_views = Vec::new();
        owned_views.try_reserve_exact(capacity).ok()?;
        Some(Self { owned_views })
    }

    unsafe fn push_borrowed_bits(&mut self, bits: u64) -> Option<*mut PyObject> {
        let view = unsafe { cext_new_pyobject_from_borrowed_bits(bits) };
        if view.is_null() {
            return None;
        }
        self.owned_views.push(view);
        Some(view)
    }

    fn push_owned_view(&mut self, view: *mut PyObject) {
        debug_assert!(!view.is_null());
        self.owned_views.push(view);
    }
}

impl Drop for CExtIngress {
    fn drop(&mut self) {
        let pending = take_native_pending_snapshot();
        for view in self.owned_views.drain(..) {
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
        }
        restore_native_pending_snapshot(pending);
    }
}

fn cext_ingress_failure(message: &str) -> i64 {
    if cpython_error_is_pending() {
        let _ = transfer_pending_cpython_exception();
        0
    } else {
        with_gil(|_py| crate::raise_exception::<i64>(&_py, "SystemError", message))
    }
}

/// Trampoline invoked by Molt's call dispatch for every registered C
/// extension function.  Signature matches Molt's
/// `extern "C" fn(closure_bits, args_ptr, args_len) -> i64`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_cext_call_trampoline(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    let _gil_call = GilGuard::new_extension_call();
    molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len)
}

#[inline(always)]
fn molt_cpython_abi_cext_call_trampoline_inner(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    // The closure encodes the registry id as a NaN-boxed int.
    let id_obj = MoltObject::from_bits(closure_bits);
    let id = match id_obj.as_int() {
        Some(value) if value >= 0 => value as usize,
        _ => {
            return with_gil(|_py| {
                crate::raise_exception::<i64>(
                    &_py,
                    "SystemError",
                    "C extension trampoline received non-int closure id",
                )
            });
        }
    };
    let entry = match cext_callable_registry().lock() {
        Ok(guard) => guard.get(id).copied(),
        Err(poisoned) => poisoned.into_inner().get(id).copied(),
    };
    let Some(entry) = entry else {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension callable registry id is out of range",
            )
        });
    };

    let n = args_len as usize;
    let args = if n == 0 {
        &[][..]
    } else if args_ptr == 0 {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension trampoline received null args pointer",
            )
        });
    } else {
        unsafe { std::slice::from_raw_parts(args_ptr as *const u64, n) }
    };

    let Some(mut ingress) = CExtIngress::with_capacity(args.len().saturating_add(2)) else {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "MemoryError",
                "failed to reserve C extension ingress views",
            )
        });
    };
    let Some(self_obj) = (unsafe { ingress.push_borrowed_bits(entry.self_bits) }) else {
        return cext_ingress_failure("failed to materialize C extension self view");
    };

    let result_pyobj = unsafe {
        match entry.dispatch_kind {
            CExtDispatchKind::NoArgs => {
                if !args.is_empty() {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "TypeError",
                            "METH_NOARGS C extension function takes no arguments",
                        )
                    });
                }
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, ptr::null_mut())
            }
            CExtDispatchKind::OneObject => {
                if args.len() != 1 {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "TypeError",
                            "METH_O C extension function takes exactly one argument",
                        )
                    });
                }
                let Some(arg) = ingress.push_borrowed_bits(args[0]) else {
                    return cext_ingress_failure("failed to materialize C extension argument view");
                };
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, arg)
            }
            CExtDispatchKind::VarArgs => {
                let Some(tuple_obj) = cext_tuple_for_args(args) else {
                    if cpython_error_is_pending() {
                        let _ = transfer_pending_cpython_exception();
                        return 0;
                    }
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    });
                };
                ingress.push_owned_view(tuple_obj);
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj)
            }
            CExtDispatchKind::VarArgsKeywords => {
                let Some(tuple_obj) = cext_tuple_for_args(args) else {
                    if cpython_error_is_pending() {
                        let _ = transfer_pending_cpython_exception();
                        return 0;
                    }
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    });
                };
                ingress.push_owned_view(tuple_obj);
                let f: PyCFunctionWithKeywords = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj, ptr::null_mut())
            }
            CExtDispatchKind::FastCall => {
                let mut fast_args = Vec::new();
                if fast_args.try_reserve_exact(args.len()).is_err() {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to reserve C extension FASTCALL arguments",
                        )
                    });
                }
                for &arg_bits in args {
                    let Some(arg) = ingress.push_borrowed_bits(arg_bits) else {
                        return cext_ingress_failure(
                            "failed to materialize C extension argument view",
                        );
                    };
                    fast_args.push(arg);
                }
                let fast_ptr = if fast_args.is_empty() {
                    ptr::null_mut()
                } else {
                    fast_args.as_mut_ptr()
                };
                let f: PyCFunctionFast = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, fast_ptr, fast_args.len() as Py_ssize_t)
            }
            CExtDispatchKind::FastCallKeywords => {
                let mut fast_args = Vec::new();
                if fast_args.try_reserve_exact(args.len()).is_err() {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to reserve C extension FASTCALL arguments",
                        )
                    });
                }
                for &arg_bits in args {
                    let Some(arg) = ingress.push_borrowed_bits(arg_bits) else {
                        return cext_ingress_failure(
                            "failed to materialize C extension argument view",
                        );
                    };
                    fast_args.push(arg);
                }
                let fast_ptr = if fast_args.is_empty() {
                    ptr::null_mut()
                } else {
                    fast_args.as_mut_ptr()
                };
                let f: PyCFunctionFastWithKeywords =
                    std::mem::transmute(entry.meth_addr as *const ());
                f(
                    self_obj,
                    fast_ptr,
                    fast_args.len() as Py_ssize_t,
                    ptr::null_mut(),
                )
            }
        }
    };

    let returned_null = result_pyobj.is_null();
    let pending = take_native_pending_snapshot();
    let call_left_error = pending.has_error();
    if !returned_null && call_left_error {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result_pyobj) };
    }
    drop(ingress);
    restore_native_pending_snapshot(pending);
    match (returned_null, call_left_error) {
        (true, true) => {
            let _ = transfer_pending_cpython_exception();
            return 0;
        }
        (true, false) => {
            return with_gil(|_py| {
                let msg = format!(
                    "C extension function returned NULL without setting an exception (convention flags 0x{:x})",
                    entry.flags
                );
                crate::raise_exception::<i64>(&_py, "SystemError", &msg)
            });
        }
        (false, true) => {
            let msg = format!(
                "C extension function returned a result with an exception set (convention flags 0x{:x})",
                entry.flags
            );
            return raise_native_result_with_error(&msg);
        }
        (false, false) => {}
    }

    let result_bits =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(result_pyobj) };
    let conversion_pending = take_native_pending_snapshot();
    let conversion_left_error = conversion_pending.has_error();
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result_pyobj) };
    if let Some(bits) = result_bits
        && conversion_left_error
    {
        unsafe { hook_dec_ref(bits) };
    }
    restore_native_pending_snapshot(conversion_pending);
    match (result_bits, conversion_left_error) {
        (Some(bits), false) => bits as i64,
        (Some(_bits), true) => raise_native_result_with_error(
            "C extension result bridge returned a value with an exception set",
        ),
        (None, true) => {
            let _ = transfer_pending_cpython_exception();
            0
        }
        (None, false) => with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension returned an object that could not enter the bridge",
            )
        }),
    }
}

#[cfg(test)]
extern "C" fn molt_cpython_abi_cext_call_trampoline_baseline(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    let _gil_call = GilGuard::new();
    molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len)
}

unsafe extern "C" fn hook_register_c_function(
    meth_addr: u64,
    flags: std::os::raw::c_int,
    self_bits: u64,
    name_data: *const u8,
    name_len: usize,
) -> u64 {
    if meth_addr == 0 || name_data.is_null() {
        return 0;
    }
    let Some(dispatch_kind) = CExtDispatchKind::from_flags(flags) else {
        return 0;
    };
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        // Reserve a registry slot for this C function.
        let id = {
            let mut guard = cext_callable_registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let id = guard.len();
            guard.push(CExtCallable {
                meth_addr: meth_addr as usize,
                flags,
                self_bits,
                dispatch_kind,
            });
            id
        };
        let closure_bits = MoltObject::from_int(id as i64).bits();
        let raw_trampoline = molt_cpython_abi_cext_call_trampoline as *const ();
        let fn_ptr_value = crate::builtins::functions::runtime_fn_addr(
            "crate::molt_cpython_abi_cext_call_trampoline",
            raw_trampoline,
        );
        let func_ptr = alloc_function_obj(&_py, fn_ptr_value, dispatch_kind.arity());
        if func_ptr.is_null() {
            return 0;
        }
        unsafe {
            #[cfg(not(target_arch = "wasm32"))]
            function_set_call_target_ptr(func_ptr, raw_trampoline);
            function_set_trampoline_ptr(func_ptr, fn_ptr_value);
            if dispatch_kind.is_variadic() {
                (*header_from_obj_ptr(func_ptr)).flags |= HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE;
            }

            // Stash __name__ on the function dict so repr() and tracebacks
            // report the C extension's actual function name.
            let name_str = alloc_string(&_py, name_bytes);
            if !name_str.is_null() {
                let name_bits = MoltObject::from_ptr(name_str).bits();
                let dict_ptr = alloc_dict_with_pairs(&_py, &[]);
                if !dict_ptr.is_null() {
                    let key_ptr = alloc_string(&_py, b"__name__");
                    if !key_ptr.is_null() {
                        let key_bits = MoltObject::from_ptr(key_ptr).bits();
                        dict_set_in_place(&_py, dict_ptr, key_bits, name_bits);
                        dec_ref_bits(&_py, key_bits);
                    }
                    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    function_set_dict_bits(func_ptr, dict_bits);
                    inc_ref_bits(&_py, dict_bits);
                    dec_ref_bits(&_py, dict_bits);
                }
                dec_ref_bits(&_py, name_bits);
            }
            // Encode the registry id into the closure slot so the
            // trampoline can recover it on every call.  Inline-int closure
            // bits are not refcounted; no inc_ref needed.
            let closure_slot = func_ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64;
            *closure_slot = closure_bits;
        }
        MoltObject::from_ptr(func_ptr).bits()
    })
}

// ─── Registration ─────────────────────────────────────────────────────────

static HOOKS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Register the runtime hooks into `molt-lang-cpython-abi`.
/// Idempotent — safe to call multiple times (only registers once).
pub fn register_cpython_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if HOOKS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    with_gil(|_py| {
        let builtins = crate::builtin_classes(&_py);
        for (class_bits, type_object) in [
            (
                builtins.object,
                (&raw mut molt_cpython_abi::abi_types::PyBaseObject_Type).cast::<PyObject>(),
            ),
            (
                builtins.type_obj,
                (&raw mut molt_cpython_abi::abi_types::PyType_Type).cast::<PyObject>(),
            ),
            (
                builtins.none_type,
                (&raw mut molt_cpython_abi::abi_types::PyNone_Type).cast::<PyObject>(),
            ),
            (
                builtins.int,
                (&raw mut molt_cpython_abi::abi_types::PyLong_Type).cast::<PyObject>(),
            ),
            (
                builtins.float,
                (&raw mut molt_cpython_abi::abi_types::PyFloat_Type).cast::<PyObject>(),
            ),
            (
                builtins.complex,
                (&raw mut molt_cpython_abi::abi_types::PyComplex_Type).cast::<PyObject>(),
            ),
            (
                builtins.bool,
                (&raw mut molt_cpython_abi::abi_types::PyBool_Type).cast::<PyObject>(),
            ),
            (
                builtins.str,
                (&raw mut molt_cpython_abi::abi_types::PyUnicode_Type).cast::<PyObject>(),
            ),
            (
                builtins.bytes,
                (&raw mut molt_cpython_abi::abi_types::PyBytes_Type).cast::<PyObject>(),
            ),
            (
                builtins.bytearray,
                (&raw mut molt_cpython_abi::abi_types::PyByteArray_Type).cast::<PyObject>(),
            ),
            (
                builtins.list,
                (&raw mut molt_cpython_abi::abi_types::PyList_Type).cast::<PyObject>(),
            ),
            (
                builtins.tuple,
                (&raw mut molt_cpython_abi::abi_types::PyTuple_Type).cast::<PyObject>(),
            ),
            (
                builtins.dict,
                (&raw mut molt_cpython_abi::abi_types::PyDict_Type).cast::<PyObject>(),
            ),
            (
                builtins.set,
                (&raw mut molt_cpython_abi::abi_types::PySet_Type).cast::<PyObject>(),
            ),
            (
                builtins.frozenset,
                (&raw mut molt_cpython_abi::abi_types::PyFrozenSet_Type).cast::<PyObject>(),
            ),
            (
                builtins.slice,
                (&raw mut molt_cpython_abi::abi_types::PySlice_Type).cast::<PyObject>(),
            ),
            (
                builtins.memoryview,
                (&raw mut molt_cpython_abi::abi_types::PyMemoryView_Type).cast::<PyObject>(),
            ),
            (
                builtins.traceback,
                (&raw mut molt_cpython_abi::abi_types::PyTraceBack_Type).cast::<PyObject>(),
            ),
            (
                builtins.module,
                (&raw mut molt_cpython_abi::abi_types::PyModule_Type).cast::<PyObject>(),
            ),
            (
                builtins.generic_alias,
                (&raw mut molt_cpython_abi::abi_types::Py_GenericAliasType).cast::<PyObject>(),
            ),
        ] {
            let bound = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(
                    type_object,
                    class_bits,
                    true,
                )
            };
            assert!(
                bound,
                "failed to bind runtime builtin class to canonical ABI type"
            );
        }
        for exception in molt_cpython_abi::abi_types::exc_singleton_ptrs() {
            let Some(c_name) = molt_cpython_abi::abi_types::exc_singleton_name(exception) else {
                continue;
            };
            let requested_name = c_name.strip_prefix("PyExc_").unwrap_or(c_name);
            let runtime_name = if requested_name == "IOError" {
                "OSError"
            } else {
                requested_name
            };
            let class_bits = crate::exception_type_bits_from_name(&_py, runtime_name);
            if class_bits == 0 {
                continue;
            }
            let canonical_view = crate::class_name_for_error(class_bits) == requested_name;
            let bound = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(
                    exception,
                    class_bits,
                    canonical_view,
                )
            };
            assert!(
                bound,
                "failed to bind {c_name} to its runtime exception class"
            );
        }
        let traceback_class_bits = crate::builtin_classes(&_py).traceback;
        let traceback_bound = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(
                (&raw mut molt_cpython_abi::abi_types::PyTraceBack_Type).cast::<PyObject>(),
                traceback_class_bits,
                true,
            )
        };
        assert!(traceback_bound, "failed to bind PyTraceBack_Type");
    });
    let hooks = RuntimeHooks {
        abi_magic: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_MAGIC,
        abi_version: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_VERSION,
        struct_size: std::mem::size_of::<RuntimeHooks>() as u32,
        gil_ensure: hook_gil_ensure,
        gil_leave: hook_gil_leave,
        gil_release: hook_gil_release,
        gil_restore: hook_gil_restore,
        gil_check: hook_gil_check,
        alloc_str: hook_alloc_str,
        alloc_bytes: hook_alloc_bytes,
        int_from_i64: hook_int_from_i64,
        int_from_u64: hook_int_from_u64,
        int_as_i64: hook_int_as_i64,
        int_as_i64_checked: hook_int_as_i64_checked,
        int_as_u64_checked: hook_int_as_u64_checked,
        int_as_u64_mask: hook_int_as_u64_mask,
        int_from_digits: hook_int_from_digits,
        int_from_f64_trunc: hook_int_from_f64_trunc,
        int_sign: hook_int_sign,
        int_signed_byte_width: hook_int_signed_byte_width,
        int_from_bytes: hook_int_from_bytes,
        int_to_bytes: hook_int_to_bytes,
        int_num_bits: hook_int_num_bits,
        int_max_str_digits: hook_int_max_str_digits,
        complex_parts: hook_complex_parts,
        complex_from_doubles: hook_complex_from_doubles,
        alloc_list: hook_alloc_list,
        alloc_list_presized: hook_alloc_list_presized,
        list_append: hook_list_append,
        list_len: hook_list_len,
        list_item: hook_list_item,
        list_set: hook_list_set,
        list_insert: hook_list_insert,
        list_sort: hook_list_sort,
        list_reverse: hook_list_reverse,
        list_set_slice: hook_list_set_slice,
        alloc_tuple: hook_alloc_tuple,
        tuple_set: hook_tuple_set,
        tuple_len: hook_tuple_len,
        tuple_item: hook_tuple_item,
        alloc_dict: hook_alloc_dict,
        dict_set: hook_dict_set,
        dict_get: hook_dict_get,
        dict_del: hook_dict_del,
        dict_len: hook_dict_len,
        dict_entry: hook_dict_entry,
        str_data: hook_str_data,
        bytes_data: hook_bytes_data,
        buffer_acquire: hook_buffer_acquire,
        buffer_release: hook_buffer_release,
        object_get_attr: hook_object_get_attr,
        object_set_attr: hook_object_set_attr,
        object_format: hook_object_format,
        float_repr: hook_float_repr,
        sys_get_object_borrowed: hook_sys_get_object_borrowed,
        eval_get_builtins_borrowed: hook_eval_get_builtins_borrowed,
        classify_heap: hook_classify_heap,
        object_hash: hook_object_hash,
        inc_ref: hook_inc_ref,
        dec_ref: hook_dec_ref,
        ref_count: hook_ref_count,
        try_mark_abi_view: hook_try_mark_abi_view,
        alloc_module: hook_alloc_module,
        module_get_dict_borrowed: hook_module_get_dict_borrowed,
        import_add_module_borrowed: hook_import_add_module_borrowed,
        module_set_attr: hook_module_set_attr,
        module_capi_register: hook_module_capi_register,
        module_capi_get_state: hook_module_capi_get_state,
        module_state_add: hook_module_state_add,
        module_state_find: hook_module_state_find,
        module_state_remove: hook_module_state_remove,
        register_c_function: hook_register_c_function,
        import_module: hook_import_module,
        exception_pending: hook_exception_pending,
        number_binary_op: hook_number_binary_op,
        number_unary_op: hook_number_unary_op,
        number_power: hook_number_power,
        dict_op: hook_dict_op,
        set_op: hook_set_op,
        set_new: hook_set_new,
        set_size: hook_set_size,
        set_contains: hook_set_contains,
        set_add: hook_set_add,
        set_discard: hook_set_discard,
        object_dir: hook_object_dir,
        object_call: hook_object_call,
        foreign_new: hook_foreign_new,
        report_unraisable: hook_report_unraisable,
        normalize_exception: hook_normalize_exception,
        exception_set_field: hook_exception_set_field,
        exception_get_field: hook_exception_get_field,
        exception_class_borrowed: hook_exception_class_borrowed,
        exception_snapshot: hook_exception_snapshot,
        exception_commit_snapshot: hook_exception_commit_snapshot,
        type_is_subtype: hook_type_is_subtype,
        take_pending_exception: hook_take_pending_exception,
        handled_exception_get: hook_handled_exception_get,
        handled_exception_set: hook_handled_exception_set,
    };
    // SAFETY: all fn pointers are valid for the process lifetime.
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{
        PyBaseExceptionObject, PyExc_IndexError, PyExc_LookupError, PyExc_RuntimeError,
        PyExc_TypeError, PyExc_ValueError, PyListObject, PyModuleDef_Base, PyModuleDef_Slot,
        PyObject, PyTypeObject,
    };
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::os::raw::c_int;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize as TestAtomicUsize};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard};
    use std::time::{Duration, Instant};

    static CANONICAL_EXEC_MODULE_BITS: AtomicU64 = AtomicU64::new(0);

    fn borrowed_bits(result: BorrowedHandleResult) -> Option<u64> {
        match result.decode() {
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => Some(bits),
            molt_cpython_abi::hooks::DecodedHandleResult::Missing
            | molt_cpython_abi::hooks::DecodedHandleResult::Error => None,
        }
    }

    #[test]
    fn exception_snapshot_commit_rejects_before_mutation_then_publishes_whole_state() {
        let _guard = cpython_abi_test_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let exception_ptr = crate::builtins::exceptions::alloc_exception(
                _py,
                "ValueError",
                "snapshot transaction",
            );
            assert!(!exception_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let before = unsafe {
                [
                    crate::exception_dict_bits(exception_ptr),
                    crate::exception_args_bits(exception_ptr),
                    crate::exception_notes_bits(exception_ptr),
                    crate::exception_trace_bits(exception_ptr),
                    crate::exception_context_bits(exception_ptr),
                    crate::exception_cause_bits(exception_ptr),
                    crate::exception_args_payload_bits(exception_ptr),
                    crate::exception_suppress_bits(exception_ptr),
                ]
            };
            let invalid_dict_ptr = alloc_string(_py, b"not a dict");
            let invalid_dict_bits = MoltObject::from_ptr(invalid_dict_ptr).bits();
            let invalid = ExceptionSnapshot {
                present_mask: EXCEPTION_SNAPSHOT_DICT | EXCEPTION_SNAPSHOT_ARGS,
                dict: invalid_dict_bits,
                args: before[1],
                ..ExceptionSnapshot::default()
            };
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const invalid) },
                -1
            );
            let after_rejection = unsafe {
                [
                    crate::exception_dict_bits(exception_ptr),
                    crate::exception_args_bits(exception_ptr),
                    crate::exception_notes_bits(exception_ptr),
                    crate::exception_trace_bits(exception_ptr),
                    crate::exception_context_bits(exception_ptr),
                    crate::exception_cause_bits(exception_ptr),
                    crate::exception_args_payload_bits(exception_ptr),
                    crate::exception_suppress_bits(exception_ptr),
                ]
            };
            assert_eq!(
                after_rejection, before,
                "rejected commit mutated exception state"
            );
            dec_ref_bits(_py, invalid_dict_bits);

            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let valid = ExceptionSnapshot {
                present_mask: EXCEPTION_SNAPSHOT_DICT | EXCEPTION_SNAPSHOT_ARGS,
                suppress_context: 1,
                dict: dict_bits,
                args: before[1],
                ..ExceptionSnapshot::default()
            };
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const valid) },
                0
            );
            assert_eq!(
                unsafe { crate::exception_dict_bits(exception_ptr) },
                dict_bits
            );
            assert!(
                crate::obj_from_bits(unsafe { crate::exception_suppress_bits(exception_ptr) })
                    .as_bool()
                    .unwrap_or(false)
            );
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, exception_bits);
        });
    }

    #[test]
    fn exception_landing_parent_projection_fully_initializes_fresh_child() {
        let _guard = cpython_abi_test_guard();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let child_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "ValueError", "fresh child");
            let parent_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "RuntimeError", "parent");
            assert!(!child_ptr.is_null() && !parent_ptr.is_null());
            let child_bits = MoltObject::from_ptr(child_ptr).bits();
            let parent_bits = MoltObject::from_ptr(parent_ptr).bits();
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    child_bits,
                )
                .expect("install fresh exception child");
            }

            let parent_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(parent_bits)
            };
            assert!(!parent_view.is_null());
            let parent_view = parent_view.cast::<PyBaseExceptionObject>();
            let child_view = unsafe { (*parent_view).context };
            assert!(!child_view.is_null());
            assert_eq!(unsafe { (*parent_view).cause }, child_view);
            let child_view = child_view.cast::<PyBaseExceptionObject>();
            assert!(
                !unsafe { (*child_view).args }.is_null(),
                "a nested fresh exception must publish its mandatory args field"
            );
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(unsafe { (*child_view).args }),
                Some(unsafe { crate::exception_args_bits(child_ptr) }),
                "the child physical args field must project its canonical runtime slot"
            );

            let none = MoltObject::none().bits();
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    none,
                )
                .expect("clear child edge");
            }
            dec_ref_bits(_py, parent_bits);
            dec_ref_bits(_py, child_bits);
        });
    }

    #[test]
    fn exception_landing_duplicate_physical_fields_release_once_at_parent_dealloc() {
        let _guard = cpython_abi_test_guard();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let child_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "ValueError", "shared child");
            let parent_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "RuntimeError", "parent");
            assert!(!child_ptr.is_null() && !parent_ptr.is_null());
            let child_bits = MoltObject::from_ptr(child_ptr).bits();
            let parent_bits = MoltObject::from_ptr(parent_ptr).bits();
            let child_baseline = unsafe {
                (*crate::header_from_obj_ptr(child_ptr))
                    .ref_count
                    .load(AtomicOrdering::Acquire)
            };
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    child_bits,
                )
                .expect("install shared child");
            }
            let parent_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(parent_bits)
            };
            assert!(!parent_view.is_null());
            let parent_view = parent_view.cast::<PyBaseExceptionObject>();
            let child_view = unsafe { (*parent_view).context };
            assert_eq!(unsafe { (*parent_view).cause }, child_view);
            assert_eq!(unsafe { (*child_view).ob_refcnt }, 3);
            assert_eq!(
                unsafe {
                    (*crate::header_from_obj_ptr(child_ptr))
                        .ref_count
                        .load(AtomicOrdering::Acquire)
                },
                child_baseline + 3,
                "two runtime fields plus one canonical child view hold"
            );

            dec_ref_bits(_py, parent_bits);
            assert_eq!(unsafe { (*child_view).ob_refcnt }, 1);
            assert_eq!(
                unsafe {
                    (*crate::header_from_obj_ptr(child_ptr))
                        .ref_count
                        .load(AtomicOrdering::Acquire)
                },
                child_baseline + 1,
                "parent dealloc must release both runtime and physical field occurrences"
            );
            dec_ref_bits(_py, child_bits);
        });
    }

    #[test]
    fn exception_landing_cyclic_projection_initializes_each_distinct_view_once() {
        let _guard = cpython_abi_test_guard();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let a_ptr = crate::builtins::exceptions::alloc_exception(_py, "ValueError", "a");
            let b_ptr = crate::builtins::exceptions::alloc_exception(_py, "TypeError", "b");
            assert!(!a_ptr.is_null() && !b_ptr.is_null());
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                a_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                b_bits,
            )
            .expect("a -> b");
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                b_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                a_bits,
            )
            .expect("b -> a");

            let a_view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(a_bits) };
            assert!(!a_view.is_null());
            let a_view = a_view.cast::<PyBaseExceptionObject>();
            let b_view = unsafe { (*a_view).context }.cast::<PyBaseExceptionObject>();
            assert!(!b_view.is_null());
            assert_eq!(unsafe { (*b_view).context }, a_view.cast::<PyObject>());
            assert!(!unsafe { (*a_view).args }.is_null());
            assert!(!unsafe { (*b_view).args }.is_null());

            let none = MoltObject::none().bits();
            for (owner, field) in [
                (
                    a_bits,
                    crate::builtins::exceptions::ExceptionFieldSlot::Context,
                ),
                (
                    b_bits,
                    crate::builtins::exceptions::ExceptionFieldSlot::Context,
                ),
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(_py, owner, field, none)
                    .expect("clear cycle edge");
            }
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);
        });
    }

    struct ForeignProxyMutation(UnsafeCell<usize>);

    unsafe impl Send for ForeignProxyMutation {}
    unsafe impl Sync for ForeignProxyMutation {}

    fn cpython_abi_test_guard() -> StdMutexGuard<'static, ()> {
        static LOCK: StdMutex<()> = StdMutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn fetched_exception_args(value: *mut PyObject) -> Vec<u64> {
        let value_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(value)
            .expect("normalized C error value must be a managed runtime exception")
            .bits();
        with_gil(|_py| {
            let value_ptr = crate::obj_from_bits(value_bits)
                .as_ptr()
                .expect("normalized exception handle must remain live");
            assert_eq!(
                unsafe { crate::object_type_id(value_ptr) },
                crate::TYPE_ID_EXCEPTION
            );
            let args_bits = crate::exception_materialized_args_bits(&_py, value_ptr);
            let args_ptr = crate::obj_from_bits(args_bits)
                .as_ptr()
                .expect("exception args must be a tuple");
            assert_eq!(
                unsafe { crate::object_type_id(args_ptr) },
                crate::TYPE_ID_TUPLE
            );
            unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                    args.to_vec()
                })
            }
            .expect("type-checked exception args tuple must remain live")
        })
    }

    #[test]
    fn c_error_normalization_uses_cpython_argument_shapes() {
        let _test_guard = cpython_abi_test_guard();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
                &raw mut molt_cpython_abi::abi_types::Py_None,
            )
        };
        let mut exc_type = ptr::null_mut();
        let mut exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        assert!(std::ptr::eq(
            exc_type,
            (&raw mut PyExc_TypeError).cast::<PyObject>()
        ));
        assert!(!exc_value.is_null());
        assert!(!std::ptr::eq(
            exc_value,
            &raw mut molt_cpython_abi::abi_types::Py_None
        ));
        assert!(fetched_exception_args(exc_value).is_empty());
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(exc_type);
            molt_cpython_abi::api::refcount::Py_DECREF(exc_value);
        }

        let tuple_bits = with_gil(|_py| {
            let tuple_ptr = crate::alloc_tuple(
                &_py,
                &[
                    MoltObject::from_int(11).bits(),
                    MoltObject::from_int(22).bits(),
                ],
            );
            assert!(!tuple_ptr.is_null());
            MoltObject::from_ptr(tuple_ptr).bits()
        });
        let tuple =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(tuple_bits) };
        assert!(!tuple.is_null());
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_ValueError).cast::<PyObject>(),
                tuple,
            );
            molt_cpython_abi::api::refcount::Py_DECREF(tuple);
        }
        exc_type = ptr::null_mut();
        exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        assert!(std::ptr::eq(
            exc_type,
            (&raw mut PyExc_ValueError).cast::<PyObject>()
        ));
        assert_eq!(
            fetched_exception_args(exc_value),
            vec![
                MoltObject::from_int(11).bits(),
                MoltObject::from_int(22).bits()
            ]
        );
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(exc_type);
            molt_cpython_abi::api::refcount::Py_DECREF(exc_value);
        }
    }

    #[test]
    fn c_error_normalization_preserves_subclass_instance_and_actual_type() {
        let _test_guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let index_handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj((&raw mut PyExc_IndexError).cast::<PyObject>())
            .expect("IndexError singleton must be bound")
            .bits();
        let type_error_handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj((&raw mut PyExc_TypeError).cast::<PyObject>())
            .expect("TypeError singleton must be bound")
            .bits();
        with_gil(|_py| {
            assert_eq!(crate::class_name_for_error(index_handle), "IndexError");
            assert_eq!(crate::class_name_for_error(type_error_handle), "TypeError");
        });
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Clear();
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut PyExc_IndexError).cast::<PyObject>(),
            );
        }
        let mut original_type = ptr::null_mut();
        let mut original_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut original_type,
                &mut original_value,
                ptr::null_mut(),
            )
        };
        let normalized_detail = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(original_value)
            .map(|handle| {
                with_gil(|_py| {
                    crate::obj_from_bits(handle.bits())
                        .as_ptr()
                        .map(|exc_ptr| {
                            format!(
                                "{}: {}",
                                crate::class_name_for_error(unsafe {
                                    crate::object_class_bits(exc_ptr)
                                }),
                                crate::format_exception_message(&_py, exc_ptr)
                            )
                        })
                        .unwrap_or_else(|| "<inline error>".to_owned())
                })
            })
            .unwrap_or_else(|| "<foreign error>".to_owned());
        assert!(
            std::ptr::eq(
                original_type,
                (&raw mut PyExc_IndexError).cast::<PyObject>()
            ),
            "PyErr_SetNone normalized to {} {:p}, expected IndexError {:p}; value={:p}; detail={}",
            molt_cpython_abi::abi_types::exc_singleton_name(original_type)
                .unwrap_or("<non-singleton>"),
            original_type,
            (&raw mut PyExc_IndexError).cast::<PyObject>(),
            original_value,
            normalized_detail,
        );
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast::<PyObject>(),
                original_value,
            );
            molt_cpython_abi::api::refcount::Py_DECREF(original_type);
        }
        let mut normalized_type = ptr::null_mut();
        let mut normalized_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut normalized_type,
                &mut normalized_value,
                ptr::null_mut(),
            )
        };
        assert!(std::ptr::eq(
            normalized_type,
            (&raw mut PyExc_IndexError).cast::<PyObject>()
        ));
        assert!(std::ptr::eq(normalized_value, original_value));
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(original_value);
            molt_cpython_abi::api::refcount::Py_DECREF(normalized_type);
            molt_cpython_abi::api::refcount::Py_DECREF(normalized_value);
        }
    }

    #[test]
    fn c_error_restore_and_managed_traceback_get_set_roundtrip_identity() {
        let _test_guard = cpython_abi_test_guard();
        register_cpython_hooks();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Clear();
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
            );
        }
        let mut exc_type = ptr::null_mut();
        let mut exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        let traceback_bits = with_gil(|_py| unsafe {
            let traceback_class = crate::builtin_classes(&_py).traceback;
            let traceback_class_ptr = crate::obj_from_bits(traceback_class)
                .as_ptr()
                .expect("traceback class must be initialized");
            crate::alloc_instance_for_class_no_pool(&_py, traceback_class_ptr)
        });
        assert!(!crate::obj_from_bits(traceback_bits).is_none());
        let traceback = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(traceback_bits)
        };
        assert!(!traceback.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::errors::PyException_SetTraceback(exc_value, traceback)
            },
            0
        );
        let direct = unsafe { molt_cpython_abi::api::errors::PyException_GetTraceback(exc_value) };
        assert!(std::ptr::eq(direct, traceback));
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(direct);
            molt_cpython_abi::api::refcount::Py_INCREF(traceback);
            molt_cpython_abi::api::errors::PyErr_Restore(exc_type, exc_value, traceback);
        }
        let mut fetched_type = ptr::null_mut();
        let mut fetched_value = ptr::null_mut();
        let mut fetched_traceback = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut fetched_type,
                &mut fetched_value,
                &mut fetched_traceback,
            )
        };
        assert!(std::ptr::eq(
            fetched_type,
            (&raw mut PyExc_TypeError).cast::<PyObject>()
        ));
        assert!(std::ptr::eq(fetched_value, exc_value));
        assert!(std::ptr::eq(fetched_traceback, traceback));
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(fetched_type);
            molt_cpython_abi::api::refcount::Py_DECREF(fetched_value);
            molt_cpython_abi::api::refcount::Py_DECREF(fetched_traceback);
            molt_cpython_abi::api::refcount::Py_DECREF(traceback);
        }
    }

    #[test]
    fn gil_custody_recursive_ensure_and_allow_threads_make_progress() {
        let _test_guard = cpython_abi_test_guard();
        register_cpython_hooks();

        let runtime_guard = GilGuard::new();
        crate::concurrency::gil::hold_runtime_gil(runtime_guard);

        let outer = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        let inner = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
            1
        );
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(inner) };
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(outer) };

        const WORKERS: usize = 8;
        const ACQUISITIONS: usize = 2_000;
        let mutation = Arc::new(ForeignProxyMutation(UnsafeCell::new(0)));
        let active = Arc::new(TestAtomicUsize::new(0));
        let max_active = Arc::new(TestAtomicUsize::new(0));
        let started = Arc::new(TestAtomicUsize::new(0));
        let finished = Arc::new(TestAtomicUsize::new(0));
        let stop_watchdog = Arc::new(AtomicBool::new(false));

        let watchdog_finished = Arc::clone(&finished);
        let watchdog_stop = Arc::clone(&stop_watchdog);
        let watchdog = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !watchdog_stop.load(AtomicOrdering::Acquire) {
                assert!(
                    Instant::now() < deadline,
                    "GIL custody stress made no progress"
                );
                if watchdog_finished.load(AtomicOrdering::Acquire) == WORKERS {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let mutation = Arc::clone(&mutation);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let started = Arc::clone(&started);
            let finished = Arc::clone(&finished);
            workers.push(std::thread::spawn(move || {
                started.fetch_add(1, AtomicOrdering::Release);
                for _ in 0..ACQUISITIONS {
                    let state = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                    assert_eq!(state, 0, "fresh worker acquisition must report unlocked");
                    let recursive = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                    assert_eq!(recursive, 1, "recursive Ensure must report locked");
                    unsafe { molt_cpython_abi::api::object::PyGILState_Release(recursive) };
                    let now = active.fetch_add(1, AtomicOrdering::AcqRel) + 1;
                    max_active.fetch_max(now, AtomicOrdering::AcqRel);
                    unsafe {
                        let value = mutation.0.get();
                        *value = (*value).wrapping_add(1);
                    }
                    active.fetch_sub(1, AtomicOrdering::AcqRel);
                    unsafe { molt_cpython_abi::api::object::PyGILState_Release(state) };
                }
                finished.fetch_add(1, AtomicOrdering::Release);
            }));
        }

        while started.load(AtomicOrdering::Acquire) != WORKERS {
            std::thread::yield_now();
        }
        while finished.load(AtomicOrdering::Acquire) != WORKERS {
            let _extension_call = GilGuard::new_extension_call();
            let saved = unsafe { molt_cpython_abi::api::object::PyEval_SaveThread() };
            std::thread::yield_now();
            unsafe { molt_cpython_abi::api::object::PyEval_RestoreThread(saved) };
        }

        {
            let _release = GilReleaseGuard::new();
            for worker in workers {
                worker.join().expect("worker panicked");
            }
        }
        stop_watchdog.store(true, AtomicOrdering::Release);
        watchdog.join().expect("watchdog panicked");

        assert_eq!(max_active.load(AtomicOrdering::Acquire), 1);
        assert_eq!(
            unsafe { *mutation.0.get() },
            WORKERS * ACQUISITIONS,
            "foreign-proxy mutation lost updates despite GIL custody"
        );
        crate::concurrency::gil::release_runtime_gil();
    }

    unsafe extern "C" fn gil_bench_noargs(
        _self: *mut PyObject,
        _args: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(
                &raw mut molt_cpython_abi::abi_types::Py_None,
            )
        };
        &raw mut molt_cpython_abi::abi_types::Py_None
    }

    #[test]
    #[ignore = "release microbenchmark"]
    fn single_thread_extension_call_preemption_bench() {
        let _test_guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let runtime_guard = GilGuard::new();
        crate::concurrency::gil::hold_runtime_gil(runtime_guard);

        let id = {
            let mut registry = cext_callable_registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let id = registry.len();
            registry.push(CExtCallable {
                meth_addr: gil_bench_noargs as *const () as usize,
                flags: METH_NOARGS,
                self_bits: MoltObject::none().bits(),
                dispatch_kind: CExtDispatchKind::NoArgs,
            });
            id
        };
        let closure_bits = MoltObject::from_int(id as i64).bits();
        const ITERATIONS: usize = 1_000_000;
        const ROUNDS: usize = 9;
        let baseline_call: extern "C" fn(u64, u64, u64) -> i64 =
            std::hint::black_box(molt_cpython_abi_cext_call_trampoline_baseline);
        let guarded_call: extern "C" fn(u64, u64, u64) -> i64 =
            std::hint::black_box(molt_cpython_abi_cext_call_trampoline);
        let mut baseline = Vec::with_capacity(ROUNDS);
        let mut guarded = Vec::with_capacity(ROUNDS);
        for round in 0..ROUNDS {
            let measure = |call: extern "C" fn(u64, u64, u64) -> i64| {
                let start = Instant::now();
                for _ in 0..ITERATIONS {
                    std::hint::black_box(call(closure_bits, 0, 0));
                }
                start.elapsed().as_nanos() as f64 / ITERATIONS as f64
            };
            if round % 2 == 0 {
                baseline.push(measure(baseline_call));
                guarded.push(measure(guarded_call));
            } else {
                guarded.push(measure(guarded_call));
                baseline.push(measure(baseline_call));
            }
        }
        baseline.sort_by(f64::total_cmp);
        guarded.sort_by(f64::total_cmp);
        let baseline_ns = baseline[ROUNDS / 2];
        let guarded_ns = guarded[ROUNDS / 2];
        let delta_pct = ((guarded_ns / baseline_ns) - 1.0) * 100.0;
        eprintln!(
            "single-thread extension call baseline={baseline_ns:.3} ns guarded={guarded_ns:.3} ns delta={delta_pct:+.3}%"
        );
        assert!(
            delta_pct < 1.0,
            "single-thread extension-call regression must stay below 1%"
        );
        crate::concurrency::gil::release_runtime_gil();
    }

    fn pending_exception_message_for_assertion() -> String {
        with_gil(|_py| {
            let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
            if MoltObject::from_bits(exc_bits).is_none() {
                return "no pending exception".to_string();
            }
            let message = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .map(|exc_ptr| crate::format_exception_message(&_py, exc_ptr))
                .unwrap_or_else(|| "pending exception handle was not a heap object".to_string());
            crate::clear_exception(&_py);
            dec_ref_bits(&_py, exc_bits);
            message
        })
    }

    fn pending_exception_type_for_assertion() -> String {
        with_gil(|_py| {
            let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
            let type_name = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .and_then(|exc_ptr| {
                    let class_bits = unsafe { crate::object_class_bits(exc_ptr) };
                    MoltObject::from_bits(class_bits)
                        .as_ptr()
                        .and_then(|class_ptr| {
                            let name_bits = unsafe { crate::class_name_bits(class_ptr) };
                            crate::string_obj_to_owned(MoltObject::from_bits(name_bits))
                        })
                })
                .unwrap_or_else(|| "<unknown>".to_string());
            dec_ref_bits(&_py, exc_bits);
            type_name
        })
    }

    static RAW_GETATTRO_CALLS: AtomicUsize = AtomicUsize::new(0);
    static mut RAW_GETATTRO_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: std::ptr::null_mut(),
    };

    unsafe extern "C" fn raw_type_getattro(
        _obj: *mut PyObject,
        _name: *mut PyObject,
    ) -> *mut PyObject {
        RAW_GETATTRO_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        &raw mut RAW_GETATTRO_RESULT
    }

    #[test]
    fn raw_c_object_getattr_bypasses_molt_value_dispatch() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        with_gil(|_py| crate::clear_exception(&_py));
        RAW_GETATTRO_CALLS.store(0, AtomicOrdering::SeqCst);

        let mut raw_type: PyTypeObject = unsafe { std::mem::zeroed() };
        raw_type.tp_name = c"numpy.ndarray".as_ptr();
        raw_type.tp_getattro = Some(raw_type_getattro);
        let mut raw_obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut raw_type,
        };
        let raw_ptr = &raw mut raw_obj;
        unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(raw_ptr);
        }

        let result = unsafe {
            molt_cpython_abi::api::object::PyObject_GetAttrString(
                raw_ptr,
                c"__array_finalize__".as_ptr(),
            )
        };

        assert_eq!(result, &raw mut RAW_GETATTRO_RESULT);
        assert_eq!(RAW_GETATTRO_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "raw C identity handles must not be decoded as Molt floats before tp_getattro"
        );
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(raw_ptr);
    }

    #[test]
    fn raw_c_type_getattr_resolves_own_type_dict() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        with_gil(|_py| crate::clear_exception(&_py));

        let mut raw_type: PyTypeObject = unsafe { std::mem::zeroed() };
        raw_type.ob_base.ob_base.ob_refcnt = 1;
        raw_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        raw_type.tp_name = c"numpy.ndarray".as_ptr();
        raw_type.tp_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
        assert!(!raw_type.tp_dict.is_null());
        let finalize = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(17) };
        assert!(!finalize.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::mapping::PyDict_SetItemString(
                    raw_type.tp_dict,
                    c"__array_finalize__".as_ptr(),
                    finalize,
                )
            },
            0
        );
        let raw_type_ptr = (&raw mut raw_type).cast::<PyObject>();
        unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(raw_type_ptr);
        }

        let result = unsafe {
            molt_cpython_abi::api::object::PyObject_GetAttrString(
                raw_type_ptr,
                c"__array_finalize__".as_ptr(),
            )
        };

        assert!(!result.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(result) },
            17
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "type_getattro must resolve the class MRO dictionary without a stale AttributeError"
        );
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(result);
            molt_cpython_abi::api::refcount::Py_DECREF(finalize);
            molt_cpython_abi::api::refcount::Py_DECREF(raw_type.tp_dict);
        }
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(raw_type_ptr);
    }

    #[test]
    fn dict_hook_set_preserves_order_hash_table_invariant_at_index_78() {
        let _guard = cpython_abi_test_guard();
        let dict_bits = unsafe { hook_alloc_dict() };
        assert_ne!(dict_bits, 0);
        let dict_ptr = MoltObject::from_bits(dict_bits).as_ptr().unwrap();

        with_gil(|_py| unsafe {
            for index in 0..78 {
                let key_bits = MoltObject::from_int(index).bits();
                let value_bits = MoltObject::from_int(index * 10).bits();
                dict_set_in_place(&_py, dict_ptr, key_bits, value_bits);
            }
        });

        let key_bits = MoltObject::from_int(78).bits();
        let value_bits = MoltObject::from_int(780).bits();
        unsafe { hook_dict_set(dict_bits, key_bits, value_bits) };

        with_gil(|_py| unsafe {
            let order = crate::builtins::containers::dict_order(dict_ptr);
            let hashes = crate::builtins::containers::dict_hashes(dict_ptr);
            let table = crate::builtins::containers::dict_table(dict_ptr);
            assert_eq!(order.len() / 2, 79);
            assert_eq!(hashes.len(), 79);
            let capacity = crate::object::ops::dict_table_capacity(79);
            crate::object::ops::dict_rebuild(&_py, order, hashes, table, capacity);
            assert_eq!(
                dict_get_in_place(&_py, dict_ptr, key_bits),
                Some(value_bits)
            );
        });
    }

    struct ModuleCacheRestore {
        name_bits: u64,
        previous_bits: u64,
    }

    impl ModuleCacheRestore {
        fn new(_py: &crate::PyToken<'_>, name_bits: u64) -> Self {
            let previous_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
            let _ = crate::molt_exception_clear();
            let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
            let _ = crate::molt_exception_clear();
            Self {
                name_bits,
                previous_bits,
            }
        }
    }

    impl Drop for ModuleCacheRestore {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(_py, {
                let _ = crate::molt_exception_clear();
                let _ = crate::builtins::modules::molt_module_cache_del(self.name_bits);
                let _ = crate::molt_exception_clear();
                if !MoltObject::from_bits(self.previous_bits).is_none() {
                    let restore_bits = crate::builtins::modules::molt_module_cache_set(
                        self.name_bits,
                        self.previous_bits,
                    );
                    if !MoltObject::from_bits(restore_bits).is_none() {
                        dec_ref_bits(_py, restore_bits);
                    }
                    let _ = crate::molt_exception_clear();
                    dec_ref_bits(_py, self.previous_bits);
                }
                dec_ref_bits(_py, self.name_bits);
            });
        }
    }

    #[test]
    fn pyimport_importmodule_routes_through_runtime_import_pipeline() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();

        let module = unsafe {
            molt_cpython_abi::api::imports::PyImport_ImportModule(
                c"molt_test_definitely_absent_module".as_ptr(),
            )
        };

        assert!(module.is_null());
        // The runtime import pipeline owns the failure: the pending error is
        // the real ModuleNotFoundError, never the standalone ABI stub text.
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("molt_test_definitely_absent_module"),
            "runtime import pipeline must name the missing module: {message}"
        );
        assert!(
            !message.contains("standalone molt-cpython-abi"),
            "registered hooks must not surface the standalone stub error: {message}"
        );
    }

    #[test]
    fn pysys_getobject_missing_sys_module_clears_speculative_import_failure() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _cache_restore = with_gil(|_py| {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            ModuleCacheRestore::new(&_py, name_bits)
        });

        let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        assert!(
            flags.is_null(),
            "PySys_GetObject(flags) must fail closed when sys is not linked"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "speculative sys import failure must not leak into the C-API caller"
        );
    }

    #[test]
    fn pysys_getobject_prefers_cached_sys_module_attribute() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();

        let (_cache_restore, expected_flags_bits, sys_module_bits) = with_gil(|_py| unsafe {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(&_py, name_bits);

            let module_ptr = alloc_module_obj(&_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let flags_ptr = crate::alloc_tuple(&_py, &[MoltObject::from_int(7).bits()]);
            assert!(!flags_ptr.is_null());
            let flags_bits = MoltObject::from_ptr(flags_ptr).bits();

            let flags_name_ptr = alloc_string(&_py, b"flags");
            assert!(!flags_name_ptr.is_null());
            let flags_name_bits = MoltObject::from_ptr(flags_name_ptr).bits();
            let dict_bits = module_dict_bits(module_ptr);
            let dict_ptr = MoltObject::from_bits(dict_bits)
                .as_ptr()
                .expect("sys module dict pointer");
            assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);
            dict_set_in_place(&_py, dict_ptr, flags_name_bits, flags_bits);
            dec_ref_bits(&_py, flags_name_bits);
            assert!(
                !crate::exception_pending(&_py),
                "test sys.flags registration must not leave an exception"
            );

            let result_bits =
                crate::builtins::modules::molt_module_cache_set(name_bits, module_bits);
            if !MoltObject::from_bits(result_bits).is_none() {
                dec_ref_bits(&_py, result_bits);
            }
            assert!(
                !crate::exception_pending(&_py),
                "test sys module registration must not leave an exception"
            );

            (cache_restore, flags_bits, module_bits)
        });

        let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        assert!(
            !flags.is_null(),
            "PySys_GetObject(flags) must resolve through the cached sys module"
        );
        let flags_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(flags)
            .map(|value| value.bits())
            .unwrap_or(0);
        assert_eq!(
            flags_bits, expected_flags_bits,
            "PySys_GetObject must prefer sys.flags over the raw flags payload"
        );
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);

        let flags_again = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        assert_eq!(flags_again, flags);
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(flags) };
        assert_eq!(unsafe { (*flags).ob_refcnt }, 2);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(flags) };
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);
        assert!(molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(flags));

        with_gil(|_py| {
            let flags_ptr = MoltObject::from_bits(expected_flags_bits)
                .as_ptr()
                .expect("sys.flags test object must remain live");
            assert_eq!(unsafe { object_type_id(flags_ptr) }, TYPE_ID_TUPLE);
            assert_eq!(unsafe { tuple_len(flags_ptr) }, 1);
            dec_ref_bits(&_py, expected_flags_bits);
            dec_ref_bits(&_py, sys_module_bits);
        });
    }

    // A cold `sys` module dict that has neither the requested attribute nor a
    // `__getattr__` must make `PySys_GetObject` fail closed (NULL) WITHOUT
    // leaking a pending exception. This guards the lazy-materialization branch
    // added to `sys_module_attr_borrowed`: when the raw dict lookup misses it
    // drives the module `__getattr__` (allow-missing) and re-reads the dict;
    // the miss path must not surface an AttributeError to the C-API caller.
    // (The real numpy path — where sys.py's PEP 562 `__getattr__` materializes
    // `flags` into the dict — is exercised by the pact-witness E2E, since the
    // compiled `sys` module is not linked into the unit-test binary.)
    #[test]
    fn pysys_getobject_missing_attr_on_cold_module_fails_closed() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();

        let (_cache_restore, sys_module_bits) = with_gil(|_py| {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(&_py, name_bits);

            let module_ptr = alloc_module_obj(&_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits =
                crate::builtins::modules::molt_module_cache_set(name_bits, module_bits);
            if !MoltObject::from_bits(result_bits).is_none() {
                dec_ref_bits(&_py, result_bits);
            }
            assert!(
                !crate::exception_pending(&_py),
                "test sys module registration must not leave an exception"
            );
            (cache_restore, module_bits)
        });

        let missing = unsafe {
            molt_cpython_abi::api::sys::PySys_GetObject(c"molt_cold_absent_attr".as_ptr())
        };
        assert!(
            missing.is_null(),
            "PySys_GetObject must fail closed for an attribute absent from a cold sys dict"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "cold-miss lazy-materialization branch must not leak a pending exception"
        );

        with_gil(|_py| {
            dec_ref_bits(&_py, sys_module_bits);
        });
    }

    #[test]
    fn cpython_abi_buffer_view_layout_matches_runtime_descriptor() {
        let _guard = cpython_abi_test_guard();
        macro_rules! assert_field {
            ($field:ident) => {
                assert_eq!(
                    std::mem::offset_of!(AbiMoltBufferView, $field),
                    std::mem::offset_of!(crate::MoltBufferView, $field),
                    concat!("MoltBufferView field offset drift: ", stringify!($field)),
                );
            };
        }

        assert_eq!(
            std::mem::size_of::<AbiMoltBufferView>(),
            std::mem::size_of::<crate::MoltBufferView>()
        );
        assert_eq!(
            std::mem::align_of::<AbiMoltBufferView>(),
            std::mem::align_of::<crate::MoltBufferView>()
        );
        assert_field!(data);
        assert_field!(len);
        assert_field!(readonly);
        assert_field!(ndim);
        assert_field!(itemsize);
        assert_field!(offset);
        assert_field!(owner);
        assert_field!(base);
        assert_field!(shape);
        assert_field!(strides);
        assert_field!(format);
    }

    #[test]
    fn pyinit_module_to_bits_accepts_static_module_def_pointer() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"static_def_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("PyModuleDef pointer must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            unsafe { hook_ref_count(bits) },
            1,
            "static PyModuleDef conversion must release its temporary C module view"
        );
        with_gil(|_py| dec_ref_bits(&_py, bits));
    }

    #[test]
    fn pyinit_module_to_bits_accepts_split_wasm_moduledef_type_clone() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut app_moduledef_type: PyTypeObject = unsafe { std::mem::zeroed() };
        app_moduledef_type.tp_name = c"moduledef".as_ptr();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &mut app_moduledef_type,
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"split_wasm_static_def_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits =
            molt_cpython_abi_pyinit_module_to_bits((&mut def as *mut PyModuleDef) as usize as u64);
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("split-WASM PyModuleDef type clone must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(unsafe { hook_ref_count(bits) }, 1);
        with_gil(|_py| dec_ref_bits(&_py, bits));
    }

    unsafe extern "C" fn canonical_exec_records_module(module_obj: *mut PyObject) -> c_int {
        if module_obj.is_null() {
            return -1;
        }
        let module_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(module_obj)
            .map(|value| value.bits())
            .unwrap_or(0);
        let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return -1;
        }
        CANONICAL_EXEC_MODULE_BITS.store(module_bits, AtomicOrdering::Relaxed);
        0
    }

    #[test]
    fn pyinit_module_to_bits_accepts_structural_module_def_without_type_marker() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        CANONICAL_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        let mut slots = [
            PyModuleDef_Slot {
                slot: 2,
                value: canonical_exec_records_module as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"source_recompiled_structural_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits =
            molt_cpython_abi_pyinit_module_to_bits((&mut def as *mut PyModuleDef) as usize as u64);
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("structural PyModuleDef must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            CANONICAL_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            bits
        );
        let def_ptr = (&mut def as *mut PyModuleDef) as usize;
        let registered_bits = crate::c_api::molt_module_state_find(def_ptr);
        if registered_bits != 0 {
            assert_eq!(registered_bits, bits);
            assert_eq!(crate::c_api::molt_module_state_remove(def_ptr), 0);
        }
    }

    unsafe extern "C" fn canonical_exec_sets_runtime_import_error(
        _module_obj: *mut PyObject,
    ) -> c_int {
        let import_error =
            with_gil(|_py| crate::exception_type_bits_from_name(&_py, "ImportError"));
        let message = b"numpy.core._multiarray_umath._ARRAY_API capsule import failed";
        unsafe {
            crate::c_api::molt_err_set(import_error, message.as_ptr(), message.len() as u64);
        }
        -1
    }

    #[test]
    fn r0_static_extension_moduledef_exec_failure_reports_module_and_rolls_back() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            PyModuleDef_Slot {
                slot: 2,
                value: canonical_exec_sets_runtime_import_error as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"moduledef_exec_error_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("moduledef_exec_error_module"), "{message}");
        assert!(
            message.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
            "{message}"
        );
        assert!(
            message.contains("numpy.core._multiarray_umath._ARRAY_API"),
            "{message}"
        );
        assert_eq!(
            crate::c_api::molt_module_state_find((&mut def as *mut PyModuleDef) as usize),
            0,
            "failed Py_mod_exec must unregister the def->module state before retry"
        );

        let retry = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);
        assert!(MoltObject::from_bits(retry).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("moduledef_exec_error_module"), "{message}");
        assert!(
            message.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
            "{message}"
        );
    }

    unsafe extern "C" fn fastcall_null_with_type_error(
        _self_obj: *mut PyObject,
        _args: *mut *mut PyObject,
        _nargs: Py_ssize_t,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
                c"numpy fastcall detail".as_ptr(),
            );
        }
        std::ptr::null_mut()
    }

    unsafe extern "C" fn fastcall_null_without_exception(
        _self_obj: *mut PyObject,
        _args: *mut *mut PyObject,
        _nargs: Py_ssize_t,
    ) -> *mut PyObject {
        std::ptr::null_mut()
    }

    #[test]
    fn cext_null_result_propagates_pending_exception() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();
        let method_bits = unsafe {
            hook_register_c_function(
                fastcall_null_with_type_error as *const () as usize as u64,
                METH_FASTCALL,
                MoltObject::none().bits(),
                b"masked_fastcall".as_ptr(),
                b"masked_fastcall".len(),
            )
        };

        let out_bits = with_gil(|_py| unsafe {
            crate::call::function::call_function_obj_bound_vec(&_py, method_bits, &[])
        });

        assert_eq!(out_bits, 0);
        assert_eq!(pending_exception_type_for_assertion(), "TypeError");
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("numpy fastcall detail"), "{message}");
        assert!(
            !message.contains("returned NULL for convention"),
            "{message}"
        );
        unsafe { hook_dec_ref(method_bits) };
    }

    #[test]
    fn cext_null_result_without_exception_raises_system_error() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();
        let method_bits = unsafe {
            hook_register_c_function(
                fastcall_null_without_exception as *const () as usize as u64,
                METH_FASTCALL,
                MoltObject::none().bits(),
                b"broken_fastcall".as_ptr(),
                b"broken_fastcall".len(),
            )
        };

        let out_bits = with_gil(|_py| unsafe {
            crate::call::function::call_function_obj_bound_vec(&_py, method_bits, &[])
        });

        assert_eq!(out_bits, 0);
        assert_eq!(pending_exception_type_for_assertion(), "SystemError");
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("returned NULL without setting an exception"),
            "{message}"
        );
        assert!(message.contains("0x80"), "{message}");
        unsafe { hook_dec_ref(method_bits) };
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_pyinit_error_state() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast::<PyObject>(),
                c"missing PyArray primitive".as_ptr(),
            );
        }

        let bits = molt_cpython_abi_pyinit_module_to_bits(0);

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("static extension PyInit returned NULL"),
            "{message}"
        );
        assert!(message.contains("missing PyArray primitive"), "{message}");
    }

    #[test]
    fn pyinit_module_to_bits_reports_invalid_handle_error_state() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: std::ptr::null(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast::<PyObject>(),
                c"module definition missing name".as_ptr(),
            );
        }

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("static extension PyInit returned an invalid module definition"),
            "{message}"
        );
        assert!(
            message.contains("module definition missing name"),
            "{message}"
        );
    }

    // ── PySet_* CPython ABI hook coverage ────────────────────────────────────
    //
    // These exercise the real set primitive end to end: the ABI `PySet_*`
    // functions route through the registered `set_*` hooks to the runtime set
    // authority (`crate::c_api::PySet_*` → hashed set object). They prove
    // membership, dedup, size, and discard semantics match CPython
    // (docs.python.org/3/c-api/set.html, Objects/setobject.c), not the prior
    // fail-closed sentinels.

    /// Wrap raw runtime handle bits into a bridge-managed `PyObject*` the ABI
    /// set functions accept as an argument.
    fn bridge_pyobj_from_bits(bits: u64) -> *mut PyObject {
        // SAFETY: handle_to_pyobj materializes a bridge PyObject entry for a
        // live runtime handle; `bits` here always comes from a hook that just
        // allocated the object, so it is valid for the bridge round-trip.
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
    }

    fn bridge_int_pyobj(value: i64) -> *mut PyObject {
        let bits = unsafe { hook_int_from_i64(value) };
        assert!(bits != 0, "hook_int_from_i64 must allocate an int");
        bridge_pyobj_from_bits(bits)
    }

    fn release_bridge_pyobj(ptr: *mut PyObject) {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(ptr);
    }

    #[test]
    fn pyset_add_contains_size_discard_round_trip() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // PySet_New(NULL) creates a real, empty runtime set — not a list.
        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(std::ptr::null_mut()) };
        assert!(!set.is_null(), "PySet_New(NULL) must return a set object");
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Check(set) },
            1,
            "PySet_New must produce a set (PySet_Check == 1)"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            0,
            "a fresh set is empty"
        );

        let key7 = bridge_int_pyobj(7);
        let key9 = bridge_int_pyobj(9);

        // Absent before add.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            0,
            "key must be absent before add"
        );

        // Add 7 → success (0), then present (1), size 1.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key7) },
            0,
            "PySet_Add success returns 0"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            1,
            "PySet_Contains after add returns 1"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1,
            "size is 1 after one add"
        );

        // Dedup: adding an equal value twice keeps size at 1.
        let key7_dup = bridge_int_pyobj(7);
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key7_dup) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1,
            "dedup: adding an equal element must not grow the set"
        );

        // Add 9 → size 2.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key9) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            2
        );

        // Discard present key → 1, then absent, size back to 1.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(set, key7) },
            1,
            "PySet_Discard of a present key returns 1"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            0,
            "discarded key is absent"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1
        );

        // Discard absent key → 0 (no error, no KeyError).
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(set, key7) },
            0,
            "PySet_Discard of an absent key returns 0"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "PySet_Discard of an absent key must not raise: {}",
            pending_exception_message_for_assertion()
        );

        release_bridge_pyobj(key7);
        release_bridge_pyobj(key7_dup);
        release_bridge_pyobj(key9);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_new_from_iterable_dedups() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // Build a list [3, 3, 5] via the runtime list authority, bridge it, and
        // feed it to PySet_New — the result must be a set of size 2 (deduped).
        let list_bits = unsafe { hook_alloc_list() };
        assert!(list_bits != 0);
        let three = unsafe { hook_int_from_i64(3) };
        let five = unsafe { hook_int_from_i64(5) };
        unsafe {
            hook_list_append(list_bits, three, std::ptr::null_mut());
            hook_list_append(list_bits, three, std::ptr::null_mut());
            hook_list_append(list_bits, five, std::ptr::null_mut());
        }
        let list = bridge_pyobj_from_bits(list_bits);

        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(list) };
        assert!(
            !set.is_null(),
            "PySet_New(iterable) must succeed: {}",
            pending_exception_message_for_assertion()
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Check(set) },
            1
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            2,
            "PySet_New from [3,3,5] dedups to {{3,5}} (size 2)"
        );

        release_bridge_pyobj(list);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_ops_fail_closed_on_non_set() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // A dict is a bridge-managed object but not a set: every mutating/query
        // op must fail closed with the CPython error sentinel + SystemError,
        // never silently succeed.
        let dict_bits = unsafe { hook_alloc_dict() };
        assert!(dict_bits != 0);
        let not_a_set = bridge_pyobj_from_bits(dict_bits);
        let key = bridge_int_pyobj(1);

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(not_a_set) },
            -1,
            "PySet_Size on a non-set returns -1"
        );
        assert!(
            with_gil(|_py| crate::exception_pending(&_py)),
            "PySet_Size on a non-set must set an exception"
        );
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(not_a_set, key) },
            -1,
            "PySet_Contains on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(not_a_set, key) },
            -1,
            "PySet_Add on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(not_a_set, key) },
            -1,
            "PySet_Discard on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        release_bridge_pyobj(key);
        release_bridge_pyobj(not_a_set);
    }

    #[test]
    fn pyset_add_unhashable_key_raises_typeerror() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(std::ptr::null_mut()) };
        assert!(!set.is_null());

        // A list is unhashable — PySet_Add must raise TypeError and return -1,
        // matching CPython, not silently drop the element.
        let list_bits = unsafe { hook_alloc_list() };
        assert!(list_bits != 0);
        let unhashable = bridge_pyobj_from_bits(list_bits);

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, unhashable) },
            -1,
            "PySet_Add of an unhashable key returns -1"
        );
        let message = pending_exception_message_for_assertion();
        assert!(
            message.to_lowercase().contains("unhashable"),
            "PySet_Add of an unhashable key must raise a TypeError mentioning 'unhashable', got: {message}"
        );

        release_bridge_pyobj(unhashable);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_stub_hooks_fail_closed() {
        // Mutation guard: the pre-init STUB_HOOKS set ops must return the
        // CPython error sentinel (0 / -1). If a future edit swapped a stub for a
        // fake-success value, this catches it before it could mask a missing
        // runtime authority.
        use molt_cpython_abi::hooks::STUB_HOOKS;
        assert_eq!(unsafe { (STUB_HOOKS.set_new)(0) }, 0);
        assert_eq!(unsafe { (STUB_HOOKS.set_size)(0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_contains)(0, 0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_add)(0, 0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_discard)(0, 0) }, -1);
    }

    #[test]
    fn hook_list_family_supports_specialized_int_and_bool_storage() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        with_gil(|_py| unsafe {
            let int_ptr = crate::object::builders::alloc_list_int_from_raw_slice(&_py, &[1, 2, 3])
                .expect("specialized int-list allocation");
            let int_bits = MoltObject::from_ptr(int_ptr).bits();
            assert_eq!(hook_classify_heap(int_bits), MoltTypeTag::List as u8);
            hook_list_append(
                int_bits,
                MoltObject::from_int(99).bits(),
                std::ptr::null_mut(),
            );
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST_INT);
            assert_eq!(hook_list_len(int_bits), 4);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 3))
                    .and_then(|bits| MoltObject::from_bits(bits).as_int()),
                Some(99)
            );

            let old = match hook_list_set(int_bits, 1, MoltObject::from_int(42).bits()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
                _ => panic!("specialized list store failed"),
            };
            assert_eq!(MoltObject::from_bits(old).as_int(), Some(2));
            dec_ref_bits(&_py, old);
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 1))
                    .and_then(|bits| MoltObject::from_bits(bits).as_int()),
                Some(42)
            );

            let bool_ptr = crate::object::builders::alloc_list_bool_from_raw_slice(&_py, &[1, 0])
                .expect("specialized bool-list allocation");
            let bool_bits = MoltObject::from_ptr(bool_ptr).bits();
            assert_eq!(hook_classify_heap(bool_bits), MoltTypeTag::List as u8);
            hook_list_append(
                bool_bits,
                MoltObject::from_bool(true).bits(),
                std::ptr::null_mut(),
            );
            assert_eq!(object_type_id(bool_ptr), TYPE_ID_LIST_BOOL);
            assert_eq!(hook_list_len(bool_bits), 3);
            assert_eq!(
                borrowed_bits(hook_list_item(bool_bits, 2))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(true)
            );

            assert_eq!(
                hook_list_set_slice(int_bits, 0, 2, bool_bits, std::ptr::null(), 0),
                0
            );
            assert_eq!(hook_list_len(int_bits), 5);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 0))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(true)
            );
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 1))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(false)
            );

            hook_tuple_set(int_bits, 0, MoltObject::from_int(7).bits(), ptr::null_mut());
            assert_eq!(borrowed_bits(hook_tuple_item(int_bits, 0)), None);
            dec_ref_bits(&_py, bool_bits);
            dec_ref_bits(&_py, int_bits);
        });
    }

    #[test]
    fn published_list_scalar_mutations_update_runtime_and_physical_views() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let list_bits = unsafe { hook_alloc_list() };
        assert_ne!(list_bits, 0);
        assert_eq!(
            unsafe {
                hook_list_append(
                    list_bits,
                    MoltObject::from_int(1).bits(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                hook_list_append(
                    list_bits,
                    MoltObject::from_int(2).bits(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let list = bridge_pyobj_from_bits(list_bits);
        let physical = list.cast::<PyListObject>();

        let physical_values = |expected_len: usize| {
            assert_eq!(
                unsafe { (*physical).ob_base.ob_size },
                expected_len as isize
            );
            (0..expected_len)
                .map(|index| {
                    let item = unsafe {
                        molt_cpython_abi::api::sequences::PyList_GetItem(list, index as isize)
                    };
                    assert!(!item.is_null());
                    unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(item) }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(physical_values(2), [1, 2]);

        crate::molt_list_append(list_bits, MoltObject::from_int(3).bits());
        assert_eq!(physical_values(3), [1, 2, 3]);

        crate::molt_list_insert(
            list_bits,
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(9).bits(),
        );
        assert_eq!(physical_values(4), [1, 9, 2, 3]);

        crate::molt_store_index(
            list_bits,
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(8).bits(),
        );
        assert_eq!(physical_values(4), [1, 9, 8, 3]);

        crate::molt_list_reverse(list_bits);
        assert_eq!(physical_values(4), [3, 8, 9, 1]);

        let popped = crate::molt_list_pop(list_bits, MoltObject::none().bits());
        assert_eq!(MoltObject::from_bits(popped).as_int(), Some(1));
        with_gil(|_py| dec_ref_bits(&_py, popped));
        assert_eq!(physical_values(3), [3, 8, 9]);

        crate::molt_list_clear(list_bits);
        assert!(physical_values(0).is_empty());
        assert!(unsafe { (*physical).ob_item.is_null() });

        let heap_bits = with_gil(|_py| {
            let ptr = alloc_string(&_py, b"heap edge");
            assert!(!ptr.is_null());
            MoltObject::from_ptr(ptr).bits()
        });
        crate::molt_list_append(list_bits, heap_bits);
        with_gil(|_py| dec_ref_bits(&_py, heap_bits));
        let list_ptr = MoltObject::from_bits(list_bits).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::tracked_heap_edge_count(list_ptr) },
            Some(1)
        );
        assert_ne!(
            unsafe { (*header_from_obj_ptr(list_ptr)).flags }
                & crate::object::HEADER_FLAG_CONTAINS_REFS,
            0
        );
        crate::molt_store_index(
            list_bits,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(42).bits(),
        );
        assert_eq!(
            unsafe { crate::object::seq_access::tracked_heap_edge_count(list_ptr) },
            Some(0)
        );
        assert_eq!(
            unsafe { (*header_from_obj_ptr(list_ptr)).flags }
                & crate::object::HEADER_FLAG_CONTAINS_REFS,
            0
        );
        assert_eq!(physical_values(1), [42]);
        crate::molt_list_clear(list_bits);

        release_bridge_pyobj(list);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pylist_append_preserves_exact_c_origin_identity() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let list_bits = unsafe { hook_alloc_list() };
        assert_ne!(list_bits, 0);
        let list = bridge_pyobj_from_bits(list_bits);
        let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        assert!(!item.is_null());

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PyList_Append(list, item) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PyList_GetItem(list, 0) },
            item,
            "PyList_Append must retain the originating object, not rematerialize an equal scalar"
        );

        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(item) };
        crate::molt_list_clear(list_bits);
        release_bridge_pyobj(list);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn projected_sequence_family_preserves_non_small_c_identity() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        use molt_cpython_abi::api::abstract_sequence::{
            _PyList_Extend, PySequence_Concat, PySequence_Contains, PySequence_Count,
            PySequence_Fast, PySequence_Fast_ITEMS, PySequence_InPlaceConcat,
            PySequence_InPlaceRepeat, PySequence_Index, PySequence_List, PySequence_Repeat,
            PySequence_Tuple,
        };
        use molt_cpython_abi::api::sequences::{
            PyList_Append, PyList_AsTuple, PyList_GetItem, PyList_GetSlice, PyList_Insert,
            PyList_New, PyList_Reverse, PyList_SetSlice, PyList_Size, PyList_Sort, PyTuple_GetItem,
            PyTuple_GetSlice, PyTuple_New, PyTuple_SetItem,
        };

        let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2000) };
        assert!(!first.is_null() && !second.is_null());

        let assert_list = |list: *mut PyObject, expected: &[*mut PyObject]| {
            assert_eq!(unsafe { PyList_Size(list) }, expected.len() as isize);
            for (index, &pointer) in expected.iter().enumerate() {
                assert_eq!(
                    unsafe { PyList_GetItem(list, index as isize) },
                    pointer,
                    "physical identity drifted at list index {index}"
                );
            }
        };

        let list = unsafe { PyList_New(0) };
        assert!(!list.is_null());
        assert_eq!(unsafe { PyList_Append(list, first) }, 0);
        assert_eq!(unsafe { PyList_Insert(list, 0, second) }, 0);
        assert_list(list, &[second, first]);
        assert_eq!(unsafe { PyList_Reverse(list) }, 0);
        assert_list(list, &[first, second]);
        assert_eq!(unsafe { PyList_Sort(list) }, 0);
        assert_list(list, &[first, second]);

        let slice = unsafe { PyList_GetSlice(list, 0, 2) };
        assert!(!slice.is_null());
        assert_list(slice, &[first, second]);

        let concat = unsafe { PySequence_Concat(list, slice) };
        assert!(!concat.is_null());
        assert_list(concat, &[first, second, first, second]);
        let repeated = unsafe { PySequence_Repeat(list, 2) };
        assert!(!repeated.is_null());
        assert_list(repeated, &[first, second, first, second]);
        let copied = unsafe { PySequence_List(list) };
        assert!(!copied.is_null());
        assert_list(copied, &[first, second]);

        let tuple = unsafe { PyList_AsTuple(list) };
        assert!(!tuple.is_null());
        assert_eq!(unsafe { PyTuple_GetItem(tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(tuple, 1) }, second);
        let sequence_tuple = unsafe { PySequence_Tuple(list) };
        assert!(!sequence_tuple.is_null());
        assert_eq!(unsafe { PyTuple_GetItem(sequence_tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(sequence_tuple, 1) }, second);

        let empty_tuple = unsafe { PyTuple_New(0) };
        let empty_tuple_again = unsafe { PyTuple_New(0) };
        assert_eq!(empty_tuple, empty_tuple_again);
        assert!(molt_cpython_abi::abi_types::is_immortal_refcnt(unsafe {
            (*empty_tuple).ob_refcnt
        }));
        let empty_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(empty_tuple)
            .expect("empty tuple is runtime-backed from birth")
            .bits();
        assert_eq!(
            unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().classify_heap)(empty_bits) },
            molt_cpython_abi::abi_types::MoltTypeTag::Tuple as u8
        );

        let c_tuple = unsafe { PyTuple_New(2) };
        assert!(!c_tuple.is_null());
        let c_tuple_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(c_tuple)
            .expect("C-created exact tuple has canonical runtime identity")
            .bits();
        assert_eq!(
            unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().tuple_len)(c_tuple_bits) },
            2
        );
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(first);
            assert_eq!(PyTuple_SetItem(c_tuple, 0, first), 0);
            molt_cpython_abi::api::refcount::Py_INCREF(second);
            assert_eq!(PyTuple_SetItem(c_tuple, 1, second), 0);
        }
        assert_eq!(unsafe { PyTuple_GetItem(c_tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(c_tuple, 1) }, second);

        let full_slice = unsafe { PyTuple_GetSlice(c_tuple, 0, 2) };
        assert_eq!(full_slice, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(full_slice) };
        let repeated_once = unsafe { PySequence_Repeat(c_tuple, 1) };
        assert_eq!(repeated_once, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(repeated_once) };
        let repeated_zero = unsafe { PySequence_Repeat(c_tuple, 0) };
        assert_eq!(repeated_zero, empty_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(repeated_zero) };
        let concat_empty = unsafe { PySequence_Concat(empty_tuple, c_tuple) };
        assert_eq!(concat_empty, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(concat_empty) };

        let fast_tuple = unsafe { PySequence_Fast(c_tuple, c"expected iterable".as_ptr()) };
        assert_eq!(fast_tuple, c_tuple);
        let fast_tuple_items = unsafe { PySequence_Fast_ITEMS(fast_tuple) };
        assert_eq!(unsafe { *fast_tuple_items }, first);
        assert_eq!(unsafe { *fast_tuple_items.add(1) }, second);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(fast_tuple) };

        let tuple_iterator = unsafe { molt_cpython_abi::api::object::PySeqIter_New(c_tuple) };
        assert!(!tuple_iterator.is_null());
        let fast_from_iterator =
            unsafe { PySequence_Fast(tuple_iterator, c"expected iterable".as_ptr()) };
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tuple_iterator) };
        assert_ne!(
            unsafe { molt_cpython_abi::api::sequences::PyList_CheckExact(fast_from_iterator) },
            0
        );
        assert_list(fast_from_iterator, &[first, second]);

        let equal_first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        assert!(!equal_first.is_null());
        assert_ne!(equal_first, first);
        let nested_left = unsafe { PyTuple_New(1) };
        let nested_right = unsafe { PyTuple_New(1) };
        let nested_list = unsafe { PyList_New(0) };
        assert!(!nested_left.is_null() && !nested_right.is_null() && !nested_list.is_null());
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(first);
            assert_eq!(PyTuple_SetItem(nested_left, 0, first), 0);
            molt_cpython_abi::api::refcount::Py_INCREF(equal_first);
            assert_eq!(PyTuple_SetItem(nested_right, 0, equal_first), 0);
        }
        assert_eq!(unsafe { PyList_Append(nested_list, nested_left) }, 0);
        assert_eq!(unsafe { PySequence_Contains(nested_list, nested_right) }, 1);
        assert_eq!(unsafe { PySequence_Count(nested_list, nested_right) }, 1);
        assert_eq!(unsafe { PySequence_Index(nested_list, nested_right) }, 0);

        let fast = unsafe { PySequence_Fast(list, c"expected iterable".as_ptr()) };
        assert_eq!(
            fast, list,
            "exact list must take PySequence_Fast's NewRef path"
        );
        let fast_items = unsafe { PySequence_Fast_ITEMS(fast) };
        assert!(!fast_items.is_null());
        assert_eq!(unsafe { *fast_items }, first);
        assert_eq!(unsafe { *fast_items.add(1) }, second);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(fast) };

        let extended = unsafe { PyList_New(0) };
        assert!(!extended.is_null());
        let none = unsafe { _PyList_Extend(extended, list) };
        assert!(!none.is_null());
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(none) };
        assert_list(extended, &[first, second]);
        assert_eq!(unsafe { PyList_SetSlice(extended, 0, 1, slice) }, 0);
        assert_list(extended, &[first, second, second]);

        let inplace = unsafe { PySequence_InPlaceConcat(extended, list) };
        assert_eq!(inplace, extended);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inplace) };
        assert_list(extended, &[first, second, second, first, second]);
        let inplace = unsafe { PySequence_InPlaceRepeat(extended, 2) };
        assert_eq!(inplace, extended);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inplace) };
        assert_list(
            extended,
            &[
                first, second, second, first, second, first, second, second, first, second,
            ],
        );

        unsafe {
            for pointer in [
                extended,
                nested_list,
                nested_right,
                nested_left,
                equal_first,
                fast_from_iterator,
                c_tuple,
                empty_tuple_again,
                empty_tuple,
                sequence_tuple,
                tuple,
                copied,
                repeated,
                concat,
                slice,
                list,
                first,
                second,
            ] {
                molt_cpython_abi::api::refcount::Py_DECREF(pointer);
            }
        }
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn list_read_and_exact_publication_share_one_cpython_authority() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        use molt_cpython_abi::abi_types::{
            PyList_Type, PyListObject, PyObject, PyTypeObject, PyVarObject,
        };
        use molt_cpython_abi::api::abstract_sequence::{
            PySequence_Concat, PySequence_List, PySequence_Repeat,
        };
        use molt_cpython_abi::api::sequences::{
            PyList_Append, PyList_Check, PyList_CheckExact, PyList_GET_ITEM, PyList_GET_SIZE,
            PyList_GetItem, PyList_GetSlice, PyList_New, PyList_Size,
        };

        let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2000) };
        let list = unsafe { PyList_New(0) };
        assert!(!first.is_null() && !second.is_null() && !list.is_null());
        assert_eq!(unsafe { PyList_Append(list, first) }, 0);
        assert_eq!(unsafe { PyList_Append(list, second) }, 0);

        let assert_exact_list = |value: *mut PyObject, expected: &[*mut PyObject]| {
            assert!(!value.is_null());
            assert_ne!(unsafe { PyList_CheckExact(value) }, 0);
            assert_eq!(unsafe { PyList_Size(value) }, expected.len() as isize);
            assert_eq!(unsafe { PyList_GET_SIZE(value) }, expected.len() as isize);
            for (index, &item) in expected.iter().enumerate() {
                assert_eq!(unsafe { PyList_GetItem(value, index as isize) }, item);
                assert_eq!(unsafe { PyList_GET_ITEM(value, index as isize) }, item);
            }
        };

        let baseline_first = unsafe { (*first).ob_refcnt };
        let baseline_second = unsafe { (*second).ob_refcnt };
        let full = unsafe { PyList_GetSlice(list, 0, isize::MAX) };
        let tail = unsafe { PyList_GetSlice(list, 1, isize::MAX) };
        let negative = unsafe { PyList_GetSlice(list, -5, -1) };
        let reversed_bounds = unsafe { PyList_GetSlice(list, 1, 0) };
        assert_exact_list(full, &[first, second]);
        assert_exact_list(tail, &[second]);
        assert_exact_list(negative, &[]);
        assert_exact_list(reversed_bounds, &[]);
        assert_ne!(full, list, "a full list C slice must be a fresh base list");

        let repeated_once = unsafe { PySequence_Repeat(list, 1) };
        let repeated_zero = unsafe { PySequence_Repeat(list, 0) };
        let repeated_negative = unsafe { PySequence_Repeat(list, -7) };
        assert_exact_list(repeated_once, &[first, second]);
        assert_exact_list(repeated_zero, &[]);
        assert_exact_list(repeated_negative, &[]);
        assert_ne!(
            repeated_once, list,
            "list repetition by one must not reuse the source"
        );

        let copied = unsafe { PySequence_List(list) };
        assert_exact_list(copied, &[first, second]);
        assert_ne!(copied, list, "PySequence_List always returns a fresh list");

        let mut subtype: PyTypeObject = unsafe { std::mem::zeroed() };
        subtype.tp_base = &raw mut PyList_Type;
        let mut subclass_items = [first, second];
        let mut subclass = PyListObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut subtype,
                },
                ob_size: 2,
            },
            ob_item: subclass_items.as_mut_ptr(),
            allocated: 2,
        };
        let subclass = (&raw mut subclass).cast::<PyObject>();
        assert_ne!(unsafe { PyList_Check(subclass) }, 0);
        assert_eq!(unsafe { PyList_Size(subclass) }, 2);
        assert_eq!(unsafe { PyList_GET_SIZE(subclass) }, 2);
        assert_eq!(unsafe { PyList_GetItem(subclass, 0) }, first);
        assert_eq!(unsafe { PyList_GET_ITEM(subclass, 1) }, second);

        let subclass_slice = unsafe { PyList_GetSlice(subclass, 0, 2) };
        let subclass_concat = unsafe { PySequence_Concat(list, subclass) };
        assert_exact_list(subclass_slice, &[first, second]);
        assert_exact_list(subclass_concat, &[first, second, first, second]);

        let mut overflow_subclass = PyListObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut subtype,
                },
                ob_size: isize::MAX,
            },
            ob_item: subclass_items.as_mut_ptr(),
            allocated: isize::MAX,
        };
        let overflow_subclass = (&raw mut overflow_subclass).cast::<PyObject>();
        assert!(unsafe { PySequence_Concat(list, overflow_subclass) }.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast::<PyObject>(),
                )
            },
            1
        );
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        unsafe {
            for value in [
                subclass_concat,
                subclass_slice,
                copied,
                repeated_negative,
                repeated_zero,
                repeated_once,
                reversed_bounds,
                negative,
                tail,
                full,
            ] {
                molt_cpython_abi::api::refcount::Py_DECREF(value);
            }
        }
        assert_eq!(unsafe { (*first).ob_refcnt }, baseline_first);
        assert_eq!(unsafe { (*second).ob_refcnt }, baseline_second);
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(list);
            molt_cpython_abi::api::refcount::Py_DECREF(first);
            molt_cpython_abi::api::refcount::Py_DECREF(second);
        }
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn integer_byte_and_bit_hooks_are_arbitrary_width_and_partial_fill_correct() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();

        let mut source = [0u8; 17];
        source[0] = 0xf0;
        source[7] = 0x12;
        source[16] = 0x01; // bit length 129
        let bits = unsafe { hook_int_from_bytes(source.as_ptr(), source.len(), 1, 0) };
        assert_ne!(bits, 0);
        let mut num_bits = 0usize;
        assert_eq!(unsafe { hook_int_num_bits(bits, &raw mut num_bits) }, 0);
        assert_eq!(num_bits, 129);

        let mut full = [0u8; 17];
        assert_eq!(
            unsafe { hook_int_to_bytes(bits, full.as_mut_ptr(), full.len(), 1, 0) },
            crate::builtins::numbers::INT_BYTES_OK
        );
        assert_eq!(full, source);

        let mut short = [0xaa; 8];
        assert_eq!(
            unsafe { hook_int_to_bytes(bits, short.as_mut_ptr(), short.len(), 1, 0) },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(short, source[..8]);

        let big_endian_source = [0x12, 0x34, 0x56];
        let big_endian = unsafe {
            hook_int_from_bytes(big_endian_source.as_ptr(), big_endian_source.len(), 0, 0)
        };
        let mut big_endian_short = [0xaa; 2];
        assert_eq!(
            unsafe {
                hook_int_to_bytes(
                    big_endian,
                    big_endian_short.as_mut_ptr(),
                    big_endian_short.len(),
                    0,
                    0,
                )
            },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(
            big_endian_short,
            [0x34, 0x56],
            "big-endian overflow writes the low bytes, not the MSB prefix"
        );

        let minus_129 = [0xff, 0x7f];
        let negative = unsafe { hook_int_from_bytes(minus_129.as_ptr(), 2, 0, 1) };
        let mut one = [0u8; 1];
        assert_eq!(
            unsafe { hook_int_to_bytes(negative, one.as_mut_ptr(), 1, 1, 1) },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(one, [0x7f]);
        assert_eq!(
            unsafe { hook_int_to_bytes(negative, one.as_mut_ptr(), 1, 1, 0) },
            crate::builtins::numbers::INT_BYTES_NEGATIVE_UNSIGNED
        );

        with_gil(|_py| {
            dec_ref_bits(&_py, bits);
            dec_ref_bits(&_py, big_endian);
            dec_ref_bits(&_py, negative);
        });
    }

    #[test]
    fn hook_tuple_set_checked_index_and_tag_guard() {
        let _guard = cpython_abi_test_guard();
        register_cpython_hooks();
        with_gil(|_py| unsafe {
            let ptr = alloc_tuple_uninitialized(&_py, 4);
            assert!(!ptr.is_null());
            assert_eq!(object_type_id(ptr), TYPE_ID_TUPLE);
            let bits = MoltObject::from_ptr(ptr).bits();

            hook_tuple_set(
                bits,
                usize::MAX,
                MoltObject::from_int(7).bits(),
                ptr::null_mut(),
            );
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );

            hook_tuple_set(
                bits,
                1_000_004,
                MoltObject::from_int(7).bits(),
                ptr::null_mut(),
            );
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );

            let val = MoltObject::from_int(42).bits();
            match hook_tuple_set(bits, 2, val, ptr::null_mut()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Missing => {}
                _ => panic!("tuple store failed"),
            }
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );
            assert_eq!(borrowed_bits(hook_tuple_item(bits, 2)), Some(val));

            let heap_ptr = alloc_string(&_py, b"owned");
            assert!(!heap_ptr.is_null());
            let heap_bits = MoltObject::from_ptr(heap_ptr).bits();
            let old = match hook_tuple_set(bits, 2, heap_bits, ptr::null_mut()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(old) => old,
                _ => panic!("tuple heap store failed"),
            };
            dec_ref_bits(&_py, old);
            assert_eq!(
                crate::object::seq_access::tracked_heap_edge_count(ptr),
                Some(1)
            );
            assert_ne!(
                (*header_from_obj_ptr(ptr)).flags & crate::object::HEADER_FLAG_CONTAINS_REFS,
                0
            );
            let old = match hook_tuple_set(bits, 2, val, ptr::null_mut()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(old) => old,
                _ => panic!("tuple primitive replacement failed"),
            };
            dec_ref_bits(&_py, old);
            assert_eq!(
                crate::object::seq_access::tracked_heap_edge_count(ptr),
                Some(0)
            );
            assert_eq!(
                (*header_from_obj_ptr(ptr)).flags & crate::object::HEADER_FLAG_CONTAINS_REFS,
                0
            );
            dec_ref_bits(&_py, heap_bits);

            let list_bits = hook_alloc_list();
            hook_tuple_set(list_bits, 0, val, ptr::null_mut());
            dec_ref_bits(&_py, list_bits);
            dec_ref_bits(&_py, bits);
        });
    }
}
