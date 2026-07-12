//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;

use molt_cpython_abi::abi_types::{
    METH_CLASS, METH_COEXIST, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O,
    METH_STATIC, METH_VARARGS, MoltTypeTag, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionWithKeywords, PyModule_Type, PyModuleDef,
    PyModuleDef_Type, PyObject, PyTypeObject,
};
use molt_cpython_abi::{MoltBufferView as AbiMoltBufferView, RuntimeHooks};
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::builtins::containers::{dict_len, dict_order, list_len, tuple_len};
use crate::builtins::numbers::{
    INT_BYTES_INVALID, bigint_from_bytes, bigint_num_bits, bigint_to_bytes, int_bits_from_bigint,
    int_bits_from_i64, int_bits_from_i128, to_bigint, to_i64,
};
use crate::concurrency::gil::with_gil;
use crate::concurrency::{GilGuard, GilReleaseGuard, gil_owned_by_current_thread};
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity,
    alloc_module_obj, alloc_string, alloc_tuple_with_capacity,
};
use crate::object::layout::{
    function_set_call_target_ptr, function_set_dict_bits, function_set_trampoline_ptr,
    module_dict_bits, seq_vec, seq_vec_ref,
};
use crate::object::ops::{
    dict_del_in_place, dict_get_in_place, dict_get_str_bytes_borrowed, dict_set_in_place,
};
use crate::object::type_ids::{
    TYPE_ID_BIGINT, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT,
    TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE,
};
use crate::object::{
    HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, MoltHeader, bytes_data, bytes_len, dec_ref_bits,
    header_from_obj_ptr, inc_ref_bits, object_type_id, string_bytes, string_len,
};

// ─── Hook implementations ─────────────────────────────────────────────────

fn abi_buffer_view_from_runtime(view: crate::MoltBufferView) -> AbiMoltBufferView {
    unsafe { std::mem::transmute::<crate::MoltBufferView, AbiMoltBufferView>(view) }
}

thread_local! {
    static ABI_GIL_ENSURE_GUARDS: std::cell::RefCell<Vec<GilGuard>> = const { std::cell::RefCell::new(Vec::new()) };
    static ABI_GIL_RELEASE_GUARDS: std::cell::RefCell<Vec<GilReleaseGuard>> = const { std::cell::RefCell::new(Vec::new()) };
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

unsafe extern "C" fn hook_int_to_bytes(
    bits: u64,
    data: *mut u8,
    len: usize,
    little_endian: c_int,
    signed: c_int,
) -> c_int {
    if data.is_null() && len != 0 {
        return INT_BYTES_INVALID;
    }
    with_gil(|_py| {
        let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
            return INT_BYTES_INVALID;
        };
        let out = if len == 0 {
            &mut [][..]
        } else {
            unsafe { std::slice::from_raw_parts_mut(data, len) }
        };
        bigint_to_bytes(&value, out, little_endian != 0, signed != 0)
    })
}

unsafe extern "C" fn hook_int_num_bits(bits: u64, out: *mut usize) -> c_int {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
            return -1;
        };
        let Some(num_bits) = bigint_num_bits(&value) else {
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

#[inline]
fn is_list_type_id(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
}

unsafe fn list_item_bits(ptr: *mut u8, i: usize) -> Option<u64> {
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_LIST => unsafe { seq_vec_ref(ptr) }.get(i).copied(),
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

unsafe fn list_bits_snapshot(ptr: *mut u8) -> Option<Vec<u64>> {
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_LIST => Some(unsafe { seq_vec_ref(ptr) }.clone()),
        TYPE_ID_LIST_INT => Some(
            unsafe { crate::object::layout::list_int_vec_ref(ptr) }
                .iter()
                .copied()
                .map(|value| MoltObject::from_int(value).bits())
                .collect(),
        ),
        TYPE_ID_LIST_BOOL => Some(
            unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
                .iter()
                .copied()
                .map(|value| MoltObject::from_bool(value != 0).bits())
                .collect(),
        ),
        _ => None,
    }
}

unsafe extern "C" fn hook_list_append(list_bits: u64, item_bits: u64) {
    // Keep representation selection, promotion, allocation accounting, and
    // element refcounting in the runtime's single list-append authority.
    let _ = crate::molt_list_append(list_bits, item_bits);
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

unsafe extern "C" fn hook_list_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe { list_item_bits(ptr, i) }.unwrap_or(0)
}

/// Indexed list store backing `PyList_SetItem`/`PyList_SET_ITEM`. Writes the
/// previous occupant's bits into `*out_old` (so the ABI can release the CPython
/// stolen-ref / `Py_SETREF` old reference) and returns 1 on success, 0 when `i`
/// is out of range or the object is not a list. O(1), allocation-free.
unsafe extern "C" fn hook_list_set(
    list_bits: u64,
    i: usize,
    val_bits: u64,
    out_old: *mut u64,
) -> i32 {
    let obj = MoltObject::from_bits(list_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    with_gil(|_py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return 0;
        }
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        let v = unsafe { seq_vec(ptr) };
        if i >= v.len() {
            return 0;
        }
        if !out_old.is_null() {
            unsafe { *out_old = v[i] };
        }
        v[i] = val_bits;
        1
    })
}

/// Insert before (clamped) index `where_` — routes to the runtime `PyList_Insert`
/// (`ins1`) authority so the shift semantics are the single source of truth.
unsafe extern "C" fn hook_list_insert(list_bits: u64, where_: isize, item_bits: u64) -> i32 {
    crate::c_api::PyList_Insert(list_bits, where_, item_bits)
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
                    unsafe { seq_vec_ref(ip) }.clone()
                }
                Some(ip) => match unsafe { list_bits_snapshot(ip) } {
                    Some(bits) => bits,
                    None => return -1,
                },
                None => return -1,
            }
        };
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        let v = unsafe { seq_vec(ptr) };
        let n = v.len() as isize;
        let low = ilow.clamp(0, n) as usize;
        let high = ihigh.clamp(low as isize, n) as usize;
        v.splice(low..high, replacement);
        0
    })
}

unsafe extern "C" fn hook_alloc_tuple(n: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_tuple_with_capacity(&_py, &[], n);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_tuple_set(bits: u64, i: usize, val_bits: u64) {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return;
    }
    let v = unsafe { seq_vec(ptr) };
    if i < v.len() {
        v[i] = val_bits;
        return;
    }
    // PyTuple_SetItem fills a pre-sized tuple. Refuse overflow and any growth
    // that would reallocate beyond that fixed construction capacity.
    let Some(new_len) = i.checked_add(1) else {
        return;
    };
    if new_len > v.capacity() {
        return;
    }
    v.resize(new_len, MoltObject::none().bits());
    v[i] = val_bits;
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

unsafe extern "C" fn hook_tuple_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return 0;
    }
    unsafe { seq_vec_ref(ptr) }.get(i).copied().unwrap_or(0)
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

unsafe extern "C" fn hook_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return;
        }
        unsafe { dict_set_in_place(&_py, ptr, key_bits, val_bits) };
    });
}

unsafe extern "C" fn hook_dict_get(dict_bits: u64, key_bits: u64) -> u64 {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return 0;
        }
        unsafe { dict_get_in_place(&_py, ptr, key_bits).unwrap_or(0) }
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

unsafe extern "C" fn hook_object_get_attr(obj_bits: u64, name_bits: u64) -> u64 {
    crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits)
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
unsafe extern "C" fn hook_object_call(callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    // Collect positional args + kwargs pairs up front. The caller (the C
    // extension via the ABI wrapper) owns references to the tuple/dict and
    // every element for the duration of the call, and the callargs builder
    // takes its own references on push.
    let mut pos: Vec<u64> = Vec::new();
    if args_bits != 0 {
        let obj = MoltObject::from_bits(args_bits);
        let Some(ptr) = obj.as_ptr() else {
            return object_call_type_error("PyObject_Call args must be a tuple");
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
            return object_call_type_error("PyObject_Call args must be a tuple");
        }
        pos.extend_from_slice(unsafe { seq_vec_ref(ptr) });
    }
    let mut kws: Vec<(u64, u64)> = Vec::new();
    if kwargs_bits != 0 {
        let obj = MoltObject::from_bits(kwargs_bits);
        let Some(ptr) = obj.as_ptr() else {
            return object_call_type_error("PyObject_Call kwargs must be a dict");
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return object_call_type_error("PyObject_Call kwargs must be a dict");
        }
        for chunk in unsafe { dict_order(ptr) }.chunks(2) {
            if chunk.len() == 2 {
                kws.push((chunk[0], chunk[1]));
            }
        }
    }
    let builder_bits = crate::molt_callargs_new(pos.len() as u64, kws.len() as u64);
    if builder_bits == 0 {
        return 0;
    }
    for &arg in &pos {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if with_gil(|_py| crate::exception_pending(&_py)) {
            return 0;
        }
    }
    for &(name, value) in &kws {
        let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, name, value) };
        if with_gil(|_py| crate::exception_pending(&_py)) {
            return 0;
        }
    }
    let result = crate::molt_call_bind(callable_bits, builder_bits);
    if with_gil(|_py| crate::exception_pending(&_py)) {
        return 0;
    }
    result
}

/// Allocate a `TYPE_ID_FOREIGN` wrapper around a genuine C-extension `PyObject*`
/// crossing INTO compiled Python. The bridge caller takes the strong reference
/// custody; this hook only materializes the Molt heap wrapper.
unsafe extern "C" fn hook_foreign_new(c_ptr: usize) -> u64 {
    with_gil(|_py| crate::object::foreign::foreign_new(&_py, c_ptr))
}

/// Raise a `TypeError` for a malformed `hook_object_call` argument shape and
/// return the hook's error sentinel (0).
fn object_call_type_error(message: &str) -> u64 {
    with_gil(|_py| {
        let _ = crate::raise_exception::<u64>(&_py, "TypeError", message);
    });
    0
}

unsafe extern "C" fn hook_object_format(obj_bits: u64, spec_bits: u64) -> u64 {
    crate::molt_format_builtin(obj_bits, spec_bits)
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
fn sys_module_attr_borrowed(attr: &[u8]) -> u64 {
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
        return 0;
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
    out
}

unsafe extern "C" fn hook_sys_get_object_borrowed(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() {
        return 0;
    }
    let name = match std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_data, name_len) })
    {
        Ok(name) => name,
        Err(_) => return 0,
    };
    sys_module_attr_borrowed(name.as_bytes())
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
        TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => MoltTypeTag::List as u8,
        TYPE_ID_TUPLE => MoltTypeTag::Tuple as u8,
        TYPE_ID_DICT => MoltTypeTag::Dict as u8,
        TYPE_ID_SET => MoltTypeTag::Set as u8,
        TYPE_ID_MODULE => MoltTypeTag::Module as u8,
        _ => MoltTypeTag::Other as u8,
    }
}

unsafe extern "C" fn hook_inc_ref(bits: u64) {
    let obj = MoltObject::from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        let hdr = ptr as *mut MoltHeader;
        if !hdr.is_null() {
            unsafe { (*hdr).ref_count.fetch_add(1, Ordering::Relaxed) };
        }
    }
}

unsafe extern "C" fn hook_dec_ref(bits: u64) {
    with_gil(|_py| dec_ref_bits(&_py, bits));
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

// ── Numeric protocol (PyNumber_*) ─────────────────────────────────────────
//
// The single numeric authority is the runtime's `PyNumber_*` compat functions
// (`crate::c_api::PyNumber_*`), which delegate to `molt_add`/`molt_pow`/etc.
// with arbitrary-precision int promotion, float coercion, operator-overload
// dispatch, and CPython-shaped exceptions. Each returns result handle bits or
// `0` with a pending runtime exception on error. These hooks are a thin routing
// layer; they perform NO arithmetic themselves.

/// Binary numeric op. `op` matches [`molt_cpython_abi::NumberBinaryOp`].
unsafe extern "C" fn hook_number_binary_op(op: u32, a_bits: u64, b_bits: u64) -> u64 {
    use molt_cpython_abi::NumberBinaryOp;
    match op {
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
    }
}

/// Unary numeric op. `op` matches [`molt_cpython_abi::NumberUnaryOp`].
unsafe extern "C" fn hook_number_unary_op(op: u32, a_bits: u64) -> u64 {
    use molt_cpython_abi::NumberUnaryOp;
    match op {
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
    }
}

/// Ternary power `pow(base, exp, modulus)`. `mod_bits == 0` means two-arg pow.
unsafe extern "C" fn hook_number_power(a_bits: u64, b_bits: u64, mod_bits: u64) -> u64 {
    crate::c_api::PyNumber_Power(a_bits, b_bits, mod_bits)
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

unsafe extern "C" fn hook_set_op(op: u32, set_bits: u64) -> u64 {
    use molt_cpython_abi::SetOp;
    match op {
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
    }
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

unsafe extern "C" fn hook_module_get_dict(module_bits: u64) -> u64 {
    with_gil(|_py| {
        let module_obj = MoltObject::from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return 0;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return 0;
        }
        unsafe { module_dict_bits(module_ptr) }
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

unsafe extern "C" fn hook_module_state_find(module_def_ptr: usize) -> u64 {
    crate::c_api::molt_module_state_find(module_def_ptr)
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

#[repr(C)]
struct StaticLinkPyMethodDef {
    ml_name: *const c_char,
    ml_meth: *mut c_void,
    ml_flags: i32,
    ml_doc: *const c_char,
}

#[repr(C)]
struct StaticLinkPyModuleDef {
    m_base: *mut c_void,
    m_name: *const c_char,
    m_doc: *const c_char,
    m_size: Py_ssize_t,
    m_methods: *mut StaticLinkPyMethodDef,
    m_slots: *mut StaticLinkPyModuleDefSlot,
    m_traverse: *mut c_void,
    m_clear: *mut c_void,
    m_free: *mut c_void,
}

#[repr(C)]
struct StaticLinkPyModuleDefSlot {
    slot: c_int,
    value: *mut c_void,
}

const STATIC_PY_MOD_CREATE: c_int = 1;
const STATIC_PY_MOD_EXEC: c_int = 2;
const STATIC_PY_MOD_MULTIPLE_INTERPRETERS: c_int = 3;
const STATIC_PY_MOD_GIL: c_int = 4;

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
        let had_exec_slot = unsafe { py_module_def_has_exec_slot(def) };
        let reason = match take_static_pyinit_error_detail() {
            Some(detail) if detail.starts_with("Py_mod_exec slot returned non-zero") => {
                format!("static-link PyModuleDef {detail}")
            }
            Some(detail) if had_exec_slot && !detail.is_empty() => {
                format!("static-link PyModuleDef Py_mod_exec slot returned non-zero: {detail}")
            }
            Some(_) if had_exec_slot => {
                "static-link PyModuleDef Py_mod_exec slot returned non-zero".to_string()
            }
            Some(detail) if !detail.is_empty() => {
                format!("static-link PyModuleDef init failed: {detail}")
            }
            _ => "static-link PyModuleDef init failed".to_string(),
        };
        return Err(format!("{module_name}: {reason}"));
    }
    let module_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(module_obj) };
    let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
        return Err(format!(
            "{module_name}: static-link PyModuleDef returned an invalid module handle"
        ));
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err(format!(
            "{module_name}: static-link PyModuleDef returned a non-module object"
        ));
    }
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

unsafe fn static_link_module_add_methods(
    module_bits: u64,
    methods: *mut StaticLinkPyMethodDef,
) -> Result<(), &'static str> {
    if methods.is_null() {
        return Ok(());
    }
    let mut cursor = methods;
    unsafe {
        while !(*cursor).ml_name.is_null() {
            let entry = &*cursor;
            if entry.ml_meth.is_null() {
                return Err("PyMethodDef method pointer must not be NULL");
            }
            let name_bytes = CStr::from_ptr(entry.ml_name).to_bytes();
            let doc_bytes = if entry.ml_doc.is_null() {
                None
            } else {
                Some(CStr::from_ptr(entry.ml_doc).to_bytes())
            };
            cext_add_py_cfunction_to_module(
                module_bits,
                name_bytes,
                entry.ml_meth as usize,
                entry.ml_flags as u32,
                doc_bytes,
            )?;
            cursor = cursor.add(1);
        }
    }
    Ok(())
}

unsafe fn static_link_module_exec_slots(
    module_bits: u64,
    slots: *mut StaticLinkPyModuleDefSlot,
    module_name: &str,
) -> Result<(), String> {
    if slots.is_null() {
        return Ok(());
    }
    let mut cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            let slot = &*cursor;
            match slot.slot {
                STATIC_PY_MOD_CREATE => {
                    return Err(format!(
                        "{module_name}: static-link PyModuleDef Py_mod_create slot requires module creation bridge"
                    ));
                }
                STATIC_PY_MOD_EXEC => {
                    if slot.value.is_null() {
                        return Err(format!(
                            "{module_name}: static-link PyModuleDef Py_mod_exec slot is NULL"
                        ));
                    }
                    type ExecFn = unsafe extern "C" fn(module: *mut PyObject) -> c_int;
                    let exec: ExecFn = std::mem::transmute(slot.value);
                    let module_obj = cext_pyobject_from_bits(module_bits);
                    if module_obj.is_null() {
                        return Err(format!(
                            "{module_name}: static-link PyModuleDef Py_mod_exec module bridge failed"
                        ));
                    }
                    let rc = exec(module_obj);
                    molt_cpython_abi::api::refcount::Py_DECREF(module_obj);
                    if rc != 0 {
                        let detail = static_pyinit_import_error_message(
                            "static-link PyModuleDef Py_mod_exec slot returned non-zero",
                        );
                        return Err(format!("{module_name}: {detail}"));
                    }
                }
                STATIC_PY_MOD_MULTIPLE_INTERPRETERS | STATIC_PY_MOD_GIL => {}
                _ => {
                    return Err(format!(
                        "{module_name}: unsupported static-link PyModuleDef slot {}",
                        slot.slot
                    ));
                }
            }
            cursor = cursor.add(1);
        }
    }
    Ok(())
}

unsafe fn static_link_module_def_to_bits(
    def: *mut StaticLinkPyModuleDef,
) -> Result<Option<u64>, String> {
    if def.is_null() {
        return Ok(None);
    }
    if unsafe { !(*def).m_base.is_null() } {
        return Ok(None);
    }
    let name = unsafe { (*def).m_name };
    if name.is_null() {
        return Ok(None);
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if name_bytes.is_empty() {
        return Err("static-link PyModuleDef name must not be empty".to_string());
    }
    let module_name = String::from_utf8_lossy(name_bytes).into_owned();
    let module_bits = unsafe { hook_alloc_module(name_bytes.as_ptr(), name_bytes.len()) };
    if module_bits == 0 {
        return Err(format!("{module_name}: module allocation failed"));
    }

    let doc = unsafe { (*def).m_doc };
    if !doc.is_null() {
        let doc_bytes = unsafe { CStr::from_ptr(doc).to_bytes() };
        let doc_bits = unsafe { hook_alloc_str(doc_bytes.as_ptr(), doc_bytes.len()) };
        if doc_bits == 0 {
            unsafe { hook_dec_ref(module_bits) };
            return Err(format!("{module_name}: doc allocation failed"));
        }
        let doc_attr = b"__doc__";
        let set_result = unsafe {
            hook_module_set_attr(module_bits, doc_attr.as_ptr(), doc_attr.len(), doc_bits)
        };
        unsafe { hook_dec_ref(doc_bits) };
        if set_result != 0 {
            unsafe { hook_dec_ref(module_bits) };
            return Err(format!("{module_name}: doc registration failed"));
        }
    }

    let module_state_size = unsafe { (*def).m_size };
    let module_state_size = if module_state_size > 0 {
        module_state_size as u64
    } else {
        0
    };
    let module_def_ptr = def as usize;
    if crate::c_api::molt_module_capi_register(module_bits, module_def_ptr, module_state_size) != 0
    {
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: C-API metadata registration failed"));
    }
    if crate::c_api::molt_module_state_add(module_bits, module_def_ptr) != 0 {
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: module-state registration failed"));
    }

    let methods = unsafe { (*def).m_methods };
    if let Err(message) = unsafe { static_link_module_add_methods(module_bits, methods) } {
        let _ = crate::c_api::molt_module_state_remove(module_def_ptr);
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: {message}"));
    }

    let slots = unsafe { (*def).m_slots };
    if let Err(message) = unsafe { static_link_module_exec_slots(module_bits, slots, &module_name) }
    {
        let _ = crate::c_api::molt_module_state_remove(module_def_ptr);
        unsafe { hook_dec_ref(module_bits) };
        return Err(message);
    }

    Ok(Some(module_bits))
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

unsafe fn py_module_def_has_exec_slot(def: *mut PyModuleDef) -> bool {
    if def.is_null() {
        return false;
    }
    let slots = unsafe { (*def).m_slots };
    if slots.is_null() {
        return false;
    }
    let mut cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            if (*cursor).slot == STATIC_PY_MOD_EXEC {
                return true;
            }
            cursor = cursor.add(1);
        }
    }
    false
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
    Some(unsafe { cext_pyobject_from_bits(spec_bits) })
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

fn take_runtime_pyinit_error_message() -> Option<String> {
    with_gil(|_py| {
        if !crate::exception_pending(&_py) {
            return None;
        }
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let message = if let Some(exc_ptr) = MoltObject::from_bits(exc_bits).as_ptr() {
            let message = crate::format_exception_message(&_py, exc_ptr);
            dec_ref_bits(&_py, exc_bits);
            message
        } else {
            "pending Molt exception handle was not a heap object".to_string()
        };
        let _ = crate::molt_exception_clear();
        Some(message)
    })
}

fn take_static_pyinit_error_detail() -> Option<String> {
    molt_cpython_abi::api::errors::take_current_error_message()
        .or_else(take_runtime_pyinit_error_message)
}

fn propagate_pending_cpython_exception(error_type: *mut PyObject) -> i64 {
    let Some((class_bits, message)) = molt_cpython_abi::api::errors::take_current_error() else {
        return 0;
    };
    with_gil(|_py| {
        if let Some(type_name) = molt_cpython_abi::abi_types::exc_singleton_name(error_type) {
            let type_name = type_name.strip_prefix("PyExc_").unwrap_or(type_name);
            return crate::raise_exception::<i64>(&_py, type_name, &message);
        }
        let message_ptr = alloc_string(&_py, message.as_bytes());
        if message_ptr.is_null() {
            return crate::raise_exception::<i64>(
                &_py,
                "MemoryError",
                "failed to materialize C extension exception message",
            );
        }
        let message_bits = MoltObject::from_ptr(message_ptr).bits();
        let args_ptr = if message.is_empty() {
            crate::alloc_tuple(&_py, &[])
        } else {
            crate::alloc_tuple(&_py, &[message_bits])
        };
        if args_ptr.is_null() {
            dec_ref_bits(&_py, message_bits);
            return crate::raise_exception::<i64>(
                &_py,
                "MemoryError",
                "failed to materialize C extension exception arguments",
            );
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let exception_ptr = crate::alloc_exception_from_class_bits(&_py, class_bits, args_bits);
        dec_ref_bits(&_py, message_bits);
        dec_ref_bits(&_py, args_bits);
        if exception_ptr.is_null() {
            return crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension set an exception with an invalid exception type",
            );
        }
        crate::builtins::exceptions::record_exception_owned(&_py, exception_ptr);
        0
    })
}

fn static_pyinit_import_error_message(prefix: &str) -> String {
    if let Some(detail) = take_static_pyinit_error_detail() {
        if detail.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}: {detail}")
        }
    } else {
        prefix.to_string()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_pyinit_module_to_bits(result_pyobj: u64) -> u64 {
    with_gil(|_py| {
        if result_pyobj == 0 {
            let message =
                static_pyinit_import_error_message("static extension PyInit returned NULL");
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        let result_ptr = result_pyobj as *mut PyObject;
        match unsafe { static_pyinit_registered_bridge_module_bits(result_ptr) } {
            Ok(Some(module_bits)) => return module_bits,
            Ok(None) => {}
            Err(message) => {
                return crate::raise_exception::<u64>(&_py, "ImportError", message);
            }
        }
        match unsafe { static_link_module_def_to_bits(result_pyobj as *mut StaticLinkPyModuleDef) }
        {
            Ok(Some(module_bits)) => return module_bits,
            Ok(None) => {}
            Err(message) => {
                return crate::raise_exception::<u64>(&_py, "ImportError", &message);
            }
        }
        if unsafe { static_pyinit_is_module_def(result_ptr) } {
            match unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) } {
                Ok(Some(module_bits)) => return module_bits,
                Ok(None) => {}
                Err(message) => {
                    return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
                }
            }
            let message = static_pyinit_import_error_message(
                "static extension PyInit returned an invalid module definition",
            );
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        if unsafe { static_pyinit_is_bridge_module_object(result_ptr) } {
            let module_bits =
                unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(result_ptr) };
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
            let message = static_pyinit_import_error_message(
                "static extension PyInit returned an invalid module handle",
            );
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        if unsafe { static_pyinit_has_module_def_shape(result_ptr) } {
            match unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) } {
                Ok(Some(module_bits)) => return module_bits,
                Ok(None) => {}
                Err(message) => {
                    return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
                }
            }
            let message = static_pyinit_import_error_message(
                "static extension PyInit returned an invalid module definition",
            );
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        let message = static_pyinit_import_error_message(
            "static extension PyInit returned an invalid module handle",
        );
        crate::raise_exception::<u64>(&_py, "ImportError", message.as_str())
    })
}

unsafe fn cext_pyobject_from_bits(bits: u64) -> *mut PyObject {
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

unsafe fn cext_tuple_for_args(args: &[u64]) -> Option<(u64, *mut PyObject)> {
    let tuple_bits = unsafe { hook_alloc_tuple(args.len()) };
    if tuple_bits == 0 {
        return None;
    }
    for (index, &arg_bits) in args.iter().enumerate() {
        unsafe { hook_tuple_set(tuple_bits, index, arg_bits) };
    }
    let tuple_obj = unsafe { cext_pyobject_from_bits(tuple_bits) };
    if tuple_obj.is_null() {
        unsafe { hook_dec_ref(tuple_bits) };
        return None;
    }
    Some((tuple_bits, tuple_obj))
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

    let mut temp_pyobjects: Vec<*mut PyObject> = Vec::new();
    let mut temp_tuple_bits: Option<u64> = None;
    let self_obj = unsafe { cext_pyobject_from_bits(entry.self_bits) };
    if !self_obj.is_null() {
        temp_pyobjects.push(self_obj);
    }

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
                let arg = cext_pyobject_from_bits(args[0]);
                temp_pyobjects.push(arg);
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, arg)
            }
            CExtDispatchKind::VarArgs => {
                let Some((tuple_bits, tuple_obj)) = cext_tuple_for_args(args) else {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    });
                };
                temp_tuple_bits = Some(tuple_bits);
                temp_pyobjects.push(tuple_obj);
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj)
            }
            CExtDispatchKind::VarArgsKeywords => {
                let Some((tuple_bits, tuple_obj)) = cext_tuple_for_args(args) else {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    });
                };
                temp_tuple_bits = Some(tuple_bits);
                temp_pyobjects.push(tuple_obj);
                let f: PyCFunctionWithKeywords = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj, ptr::null_mut())
            }
            CExtDispatchKind::FastCall => {
                let mut fast_args = Vec::with_capacity(args.len());
                for &arg_bits in args {
                    let arg = cext_pyobject_from_bits(arg_bits);
                    temp_pyobjects.push(arg);
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
                let mut fast_args = Vec::with_capacity(args.len());
                for &arg_bits in args {
                    let arg = cext_pyobject_from_bits(arg_bits);
                    temp_pyobjects.push(arg);
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

    let result_bits = if result_pyobj.is_null() {
        None
    } else {
        Some(unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(result_pyobj) })
    };
    for temp in temp_pyobjects {
        unsafe { molt_cpython_abi::api::refcount::Py_XDECREF(temp) };
    }
    if let Some(tuple_bits) = temp_tuple_bits {
        unsafe { hook_dec_ref(tuple_bits) };
    }
    match result_bits {
        Some(bits) => bits as i64,
        None => {
            let error_type = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
            if !error_type.is_null() {
                propagate_pending_cpython_exception(error_type)
            } else {
                with_gil(|_py| {
                    let msg = format!(
                        "C extension function returned NULL without setting an exception (convention flags 0x{:x})",
                        entry.flags
                    );
                    crate::raise_exception::<i64>(&_py, "SystemError", &msg)
                })
            }
        }
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
    let hooks = RuntimeHooks {
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
        int_from_bytes: hook_int_from_bytes,
        int_to_bytes: hook_int_to_bytes,
        int_num_bits: hook_int_num_bits,
        int_max_str_digits: hook_int_max_str_digits,
        alloc_list: hook_alloc_list,
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
        classify_heap: hook_classify_heap,
        inc_ref: hook_inc_ref,
        dec_ref: hook_dec_ref,
        alloc_module: hook_alloc_module,
        module_get_dict: hook_module_get_dict,
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
        PyExc_RuntimeError, PyExc_TypeError, PyModuleDef_Base, PyModuleDef_Slot, PyObject,
        PyTypeObject,
    };
    use std::cell::UnsafeCell;
    use std::os::raw::c_int;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize as TestAtomicUsize};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard};
    use std::time::{Duration, Instant};

    static STATIC_LINK_EXEC_MODULE_BITS: AtomicU64 = AtomicU64::new(0);

    struct ForeignProxyMutation(UnsafeCell<usize>);

    unsafe impl Send for ForeignProxyMutation {}
    unsafe impl Sync for ForeignProxyMutation {}

    fn cpython_abi_test_guard() -> StdMutexGuard<'static, ()> {
        static LOCK: StdMutex<()> = StdMutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
                    let class_bits = unsafe { crate::exception_class_bits(exc_ptr) };
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
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.register_raw_pyobj(raw_ptr);
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
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.register_raw_pyobj(raw_type_ptr);
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

            let flags_ptr = alloc_tuple_with_capacity(&_py, &[MoltObject::from_int(7).bits()], 1);
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
        let flags_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(flags) };
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
    }

    #[test]
    fn pyinit_module_to_bits_accepts_static_link_compact_module_def_without_methods_or_slots() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_compact_module".as_ptr(),
            m_doc: c"compact static-link module".as_ptr(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("compact static-link PyModuleDef must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            crate::c_api::molt_module_state_find((&mut def as *mut StaticLinkPyModuleDef) as usize),
            bits
        );
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    unsafe extern "C" fn static_link_exec_records_module(module_obj: *mut PyObject) -> c_int {
        if module_obj.is_null() {
            return -1;
        }
        let module_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(module_obj) };
        let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return -1;
        }
        STATIC_LINK_EXEC_MODULE_BITS.store(module_bits, AtomicOrdering::Relaxed);
        0
    }

    #[test]
    fn pyinit_module_to_bits_accepts_structural_module_def_without_type_marker() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        STATIC_LINK_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        let mut slots = [
            PyModuleDef_Slot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_records_module as *mut c_void,
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
            STATIC_LINK_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            bits
        );
        let def_ptr = (&mut def as *mut PyModuleDef) as usize;
        let registered_bits = crate::c_api::molt_module_state_find(def_ptr);
        if registered_bits != 0 {
            assert_eq!(registered_bits, bits);
            assert_eq!(crate::c_api::molt_module_state_remove(def_ptr), 0);
        }
    }

    #[test]
    fn pyinit_module_to_bits_executes_static_link_py_mod_exec_and_metadata_slots() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        STATIC_LINK_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_records_module as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_MULTIPLE_INTERPRETERS,
                value: 2usize as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_GIL,
                value: std::ptr::dangling_mut::<c_void>(),
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_exec_slot_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("static-link Py_mod_exec module must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            STATIC_LINK_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            bits
        );
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    #[test]
    fn pyinit_module_to_bits_rejects_static_link_py_mod_create_slot() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_CREATE,
                value: std::ptr::dangling_mut::<c_void>(),
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_create_slot_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains(
            "static-link PyModuleDef Py_mod_create slot requires module creation bridge"
        ));
    }

    unsafe extern "C" fn static_link_exec_sets_runtime_error(_module_obj: *mut PyObject) -> c_int {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_RuntimeError,
                c"missing PyArray dtype bootstrap".as_ptr(),
            );
        }
        -1
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_sets_runtime_error as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_exec_error_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("static_link_exec_error_module"));
        assert!(message.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"));
        assert!(message.contains("missing PyArray dtype bootstrap"));
    }

    unsafe extern "C" fn static_link_exec_sets_runtime_import_error(
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
    fn pyinit_module_to_bits_reports_static_link_py_mod_exec_runtime_error() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_sets_runtime_import_error as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_runtime_error_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("static_link_runtime_error_module"));
        assert!(message.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"));
        assert!(message.contains("numpy.core._multiarray_umath._ARRAY_API"));
    }

    #[test]
    fn r0_static_extension_moduledef_exec_failure_reports_module_and_rolls_back() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            PyModuleDef_Slot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_sets_runtime_import_error as *mut c_void,
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

    unsafe extern "C" fn pyobject_bridge_tuple_len_method(
        _self_obj: *mut PyObject,
        args_obj: *mut PyObject,
    ) -> *mut PyObject {
        if args_obj.is_null() {
            return std::ptr::null_mut();
        }
        let args_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(args_obj) };
        let Some(args_ptr) = MoltObject::from_bits(args_bits).as_ptr() else {
            return std::ptr::null_mut();
        };
        if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
            return std::ptr::null_mut();
        }
        let len = unsafe { tuple_len(args_ptr) };
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(len as std::os::raw::c_long) }
    }

    unsafe extern "C" fn fastcall_null_with_type_error(
        _self_obj: *mut PyObject,
        _args: *mut *mut PyObject,
        _nargs: Py_ssize_t,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_TypeError,
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
    fn pyinit_module_to_bits_registers_static_link_methods_through_pyobject_bridge() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut methods = [
            StaticLinkPyMethodDef {
                ml_name: c"arg_count".as_ptr(),
                ml_meth: pyobject_bridge_tuple_len_method as *mut c_void,
                ml_flags: METH_VARARGS,
                ml_doc: c"return positional argument count".as_ptr(),
            },
            StaticLinkPyMethodDef {
                ml_name: std::ptr::null(),
                ml_meth: std::ptr::null_mut(),
                ml_flags: 0,
                ml_doc: std::ptr::null(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_pyobject_method_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: methods.as_mut_ptr(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("static-link PyMethodDef module must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);

        let method_bits =
            unsafe { crate::c_api::molt_module_get_object_bytes(bits, b"arg_count".as_ptr(), 9) };
        assert!(!MoltObject::from_bits(method_bits).is_none());
        let args_bits = unsafe { hook_alloc_tuple(3) };
        assert_ne!(args_bits, 0);
        unsafe {
            hook_tuple_set(args_bits, 0, MoltObject::from_int(1).bits());
            hook_tuple_set(args_bits, 1, MoltObject::from_int(2).bits());
            hook_tuple_set(args_bits, 2, MoltObject::from_int(3).bits());
        }

        let direct_out_bits = with_gil(|_py| unsafe {
            crate::call::function::call_function_obj_bound_vec(
                &_py,
                method_bits,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            )
        });
        assert_eq!(
            to_i64(MoltObject::from_bits(direct_out_bits)),
            Some(3),
            "direct static-link PyMethodDef trampoline failed: {}",
            pending_exception_message_for_assertion()
        );
        unsafe { hook_dec_ref(direct_out_bits) };

        let out_bits =
            crate::c_api::molt_object_call(method_bits, args_bits, MoltObject::none().bits());

        assert_eq!(
            to_i64(MoltObject::from_bits(out_bits)),
            Some(3),
            "public object-call route for static-link PyMethodDef failed: {}",
            pending_exception_message_for_assertion()
        );

        unsafe {
            hook_dec_ref(out_bits);
            hook_dec_ref(args_bits);
            hook_dec_ref(method_bits);
        }
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_pyinit_error_state() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_RuntimeError,
                c"missing PyArray primitive".as_ptr(),
            );
        }

        let bits = molt_cpython_abi_pyinit_module_to_bits(0);

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("static extension PyInit returned NULL"));
        assert!(message.contains("missing PyArray primitive"));
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
                &raw mut PyExc_RuntimeError,
                c"module definition missing name".as_ptr(),
            );
        }

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("static extension PyInit returned an invalid module definition"));
        assert!(message.contains("module definition missing name"));
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
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_pyobj(bits) }
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
            hook_list_append(list_bits, three);
            hook_list_append(list_bits, three);
            hook_list_append(list_bits, five);
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
            hook_list_append(int_bits, MoltObject::from_int(99).bits());
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST_INT);
            assert_eq!(hook_list_len(int_bits), 4);
            assert_eq!(
                MoltObject::from_bits(hook_list_item(int_bits, 3)).as_int(),
                Some(99)
            );

            let mut old = 0;
            assert_eq!(
                hook_list_set(int_bits, 1, MoltObject::from_int(42).bits(), &mut old),
                1
            );
            assert_eq!(MoltObject::from_bits(old).as_int(), Some(2));
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST);
            assert_eq!(
                MoltObject::from_bits(hook_list_item(int_bits, 1)).as_int(),
                Some(42)
            );

            let bool_ptr = crate::object::builders::alloc_list_bool_from_raw_slice(&_py, &[1, 0])
                .expect("specialized bool-list allocation");
            let bool_bits = MoltObject::from_ptr(bool_ptr).bits();
            assert_eq!(hook_classify_heap(bool_bits), MoltTypeTag::List as u8);
            hook_list_append(bool_bits, MoltObject::from_bool(true).bits());
            assert_eq!(object_type_id(bool_ptr), TYPE_ID_LIST_BOOL);
            assert_eq!(hook_list_len(bool_bits), 3);
            assert_eq!(
                MoltObject::from_bits(hook_list_item(bool_bits, 2)).as_bool(),
                Some(true)
            );

            assert_eq!(hook_list_set_slice(int_bits, 0, 2, bool_bits), 0);
            assert_eq!(hook_list_len(int_bits), 5);
            assert_eq!(
                MoltObject::from_bits(hook_list_item(int_bits, 0)).as_bool(),
                Some(true)
            );
            assert_eq!(
                MoltObject::from_bits(hook_list_item(int_bits, 1)).as_bool(),
                Some(false)
            );

            hook_tuple_set(int_bits, 0, MoltObject::from_int(7).bits());
            assert_eq!(hook_tuple_item(int_bits, 0), 0);
            dec_ref_bits(&_py, bool_bits);
            dec_ref_bits(&_py, int_bits);
        });
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
            let ptr = alloc_tuple_with_capacity(&_py, &[], 4);
            assert!(!ptr.is_null());
            assert_eq!(object_type_id(ptr), TYPE_ID_TUPLE);
            let bits = MoltObject::from_ptr(ptr).bits();
            let cap = seq_vec_ref(ptr).capacity();

            hook_tuple_set(bits, usize::MAX, MoltObject::from_int(7).bits());
            assert_eq!(seq_vec_ref(ptr).len(), 0);

            hook_tuple_set(bits, cap + 1_000_000, MoltObject::from_int(7).bits());
            assert_eq!(seq_vec_ref(ptr).len(), 0);
            assert_eq!(seq_vec_ref(ptr).capacity(), cap);

            let val = MoltObject::from_int(42).bits();
            hook_tuple_set(bits, 2, val);
            assert_eq!(seq_vec_ref(ptr).len(), 3);
            assert_eq!(hook_tuple_item(bits, 2), val);

            let list_bits = hook_alloc_list();
            hook_tuple_set(list_bits, 0, val);
            dec_ref_bits(&_py, list_bits);
            dec_ref_bits(&_py, bits);
        });
    }
}
