//! CPython-compatible `Py*` C API runtime helpers.
//!
//! Boxed-handle operations shared by runtime hooks and runtime tests. The
//! exported pointer-based C symbol authority lives in `molt-cpython-abi`.

use super::*;
use crate::BuiltinExc;

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — Dict direct access
// ---------------------------------------------------------------------------

/// All libmolt C dict operations share the runtime's tri-state backing query.
pub(super) fn c_dict_pointer(py: &PyToken<'_>, dict: u64) -> Option<*mut u8> {
    match crate::object::ops::dict_backing_bits(py, dict) {
        Ok(Some(bits)) => obj_from_bits(bits).as_ptr(),
        Ok(None) => {
            raise_exception::<()>(
                py,
                BuiltinExc::SystemError,
                "bad argument to internal function",
            );
            None
        }
        Err(()) => None,
    }
}

/// `PyDict_GetItem(dict, key)` — return a **borrowed** reference to dict[key],
/// or 0 (NULL) if the key is not present (no exception set for missing key).
pub extern "C" fn PyDict_GetItem(dict: u64, key: u64) -> u64 {
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(ptr) = c_dict_pointer(_py, dict) else {
                return 0;
            };
            unsafe { dict_get_in_place(_py, ptr, key).ok().flatten().unwrap_or(0) }
        })
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Number Protocol
// ---------------------------------------------------------------------------

/// `PyNumber_Add(a, b)` — return `a + b`, or 0 on error.
pub extern "C" fn PyNumber_Add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_add(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Subtract(a, b)` — return `a - b`, or 0 on error.
pub extern "C" fn PyNumber_Subtract(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_sub(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Multiply(a, b)` — return `a * b`, or 0 on error.
pub extern "C" fn PyNumber_Multiply(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_mul(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_MatrixMultiply(a, b)` — return `a @ b`, or 0 on error.
pub extern "C" fn PyNumber_MatrixMultiply(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_matmul(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_TrueDivide(a, b)` — return `a / b`, or 0 on error.
pub extern "C" fn PyNumber_TrueDivide(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_div(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_FloorDivide(a, b)` — return `a // b`, or 0 on error.
pub extern "C" fn PyNumber_FloorDivide(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_floordiv(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Remainder(a, b)` — return `a % b`, or 0 on error.
pub extern "C" fn PyNumber_Remainder(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_mod(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Negative(a)` — return `-a`, or 0 on error.
pub extern "C" fn PyNumber_Negative(a: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_operator_neg(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Positive(a)` — return `+a`, or 0 on error.
pub extern "C" fn PyNumber_Positive(a: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_operator_pos(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Absolute(a)` — return `abs(a)`, or 0 on error.
pub extern "C" fn PyNumber_Absolute(a: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_abs_builtin(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Invert(a)` — return `~a`, or 0 on error.
pub extern "C" fn PyNumber_Invert(a: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_invert(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Lshift(a, b)` — return `a << b`, or 0 on error.
pub extern "C" fn PyNumber_Lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_lshift(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Rshift(a, b)` — return `a >> b`, or 0 on error.
pub extern "C" fn PyNumber_Rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_rshift(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_And(a, b)` — return `a & b`, or 0 on error.
pub extern "C" fn PyNumber_And(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_bit_and(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Or(a, b)` — return `a | b`, or 0 on error.
pub extern "C" fn PyNumber_Or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_bit_or(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Xor(a, b)` — return `a ^ b`, or 0 on error.
pub extern "C" fn PyNumber_Xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_bit_xor(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Mapping Protocol
// ---------------------------------------------------------------------------

/// `PyMapping_Keys(o)` — return `list(o.keys())`, or 0 on error.
pub extern "C" fn PyMapping_Keys(o: u64) -> u64 {
    mapping_method_as_list(o, c"keys")
}

/// `PyMapping_Values(o)` — return `list(o.values())`, or 0 on error.
pub extern "C" fn PyMapping_Values(o: u64) -> u64 {
    mapping_method_as_list(o, c"values")
}

/// `PyMapping_Items(o)` — return `list(o.items())`, or 0 on error.
pub extern "C" fn PyMapping_Items(o: u64) -> u64 {
    mapping_method_as_list(o, c"items")
}

/// Mapping methods are ordinary attribute calls; subclasses and arbitrary
/// mappings may override them. Only the returned iterable is materialized.
fn mapping_method_as_list(o: u64, name: &std::ffi::CStr) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let method = PyObject_GetAttrString(o, name.as_ptr());
        if exception_pending(_py) {
            dec_ref_bits(_py, method);
            return 0;
        }
        let result = unsafe { call_callable0(_py, method) };
        dec_ref_bits(_py, method);
        crate::object::mapping_merge::output_as_list(_py, o, name.to_bytes(), result).unwrap_or(0)
    })
}

fn dict_output_as_list(dict: u64, kind: crate::object::ops_dict::DictSnapshotKind) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = c_dict_pointer(_py, dict) else {
            return 0;
        };
        let Some(snapshot) =
            (unsafe { crate::object::ops_dict::dict_snapshot(_py, storage, kind) })
        else {
            return 0;
        };
        let list = crate::object::builders::alloc_list_from_snapshot(_py, snapshot);
        if list.is_null() {
            0
        } else {
            MoltObject::from_ptr(list).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Object Protocol (PyObject_*)
// ---------------------------------------------------------------------------

/// `PyObject_GetAttrString(obj, name)` — return obj.name using a C string, or 0 on error.
pub extern "C" fn PyObject_GetAttrString(obj: u64, name: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if name.is_null() {
            let _ =
                raise_exception::<u64>(_py, BuiltinExc::TypeError, "attribute name cannot be null");
            return 0;
        }
        let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        let name_bytes = name_cstr.to_bytes();
        let name_ptr = alloc_string(_py, name_bytes);
        if name_ptr.is_null() {
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let result = molt_object_getattr(obj, name_bits);
        dec_ref_bits(_py, name_bits);
        result
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Set Protocol
// ---------------------------------------------------------------------------

/// One set/frozenset construction owner; absence is independent of value bits.
pub(crate) fn new_set_from_iterable(iterable: Option<u64>, frozen: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let result = if frozen {
            molt_frozenset_new(0)
        } else {
            molt_set_new(0)
        };
        if obj_from_bits(result).is_none() {
            return 0;
        }
        if let Some(iterable) = iterable {
            let Some(pointer) = obj_from_bits(result).as_ptr() else {
                return 0;
            };
            if unsafe {
                crate::object::ops_set::set_update_iterable(
                    _py,
                    pointer,
                    iterable,
                    crate::object::ops_hash::HashContext::SetElement,
                )
            }
            .is_err()
            {
                dec_ref_bits(_py, result);
                return 0;
            }
        }
        result
    })
}

/// Return `true` when `bits` is a `set` or `frozenset` heap object.
///
/// CPython's set-protocol functions raise `SystemError` (via the
/// `PyAnySet_Check` / `PySet_Check` guards in `Objects/setobject.c`) when
/// handed a non-set. Faking a size of 0, "not present", or "success" for a
/// non-set object would silently corrupt caller logic, so the wrappers below
/// gate on this and fail closed with `SystemError` — matching CPython.
fn is_any_set_bits(bits: u64) -> bool {
    match obj_from_bits(bits).as_ptr() {
        Some(ptr) => {
            let tid = unsafe { object_type_id(ptr) };
            tid == TYPE_ID_SET || tid == TYPE_ID_FROZENSET
        }
        None => false,
    }
}

/// `PySet_Size(anyset)` — return the number of elements in the set/frozenset.
/// Returns -1 with `SystemError` set if `anyset` is not a set or frozenset.
pub extern "C" fn PySet_Size(set: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        if !is_any_set_bits(set) {
            let _ = raise_exception::<u64>(
                _py,
                BuiltinExc::SystemError,
                "PySet_Size: expected a set or frozenset",
            );
            return -1;
        }
        unsafe { crate::set_len(obj_from_bits(set).as_ptr().unwrap()) as isize }
    })
}

/// `PySet_Contains(anyset, key)` — return 1 if key is in set, 0 if not, -1 on error.
/// Raises `SystemError` if `anyset` is not a set/frozenset, `TypeError` if `key`
/// is unhashable (both surfaced as -1 with the exception set).
pub extern "C" fn PySet_Contains(set: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if !is_any_set_bits(set) {
            let _ = raise_exception::<u64>(
                _py,
                BuiltinExc::SystemError,
                "PySet_Contains: expected a set or frozenset",
            );
            return -1;
        }
        crate::object::ops_set::set_contains(
            _py,
            set,
            key,
            crate::object::ops_set::SetContainsPolicy::ExactKey,
        )
        .map(i32::from)
        .unwrap_or(-1)
    })
}

/// `PySet_Add(set, key)` — add key to set. Returns 0 on success, -1 on error.
/// Raises `SystemError` if `set` is not a set/frozenset, `TypeError` if `key`
/// is unhashable (both surfaced as -1 with the exception set).
pub extern "C" fn PySet_Add(set: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        // CPython PySet_Add accepts set or frozenset (frozenset only during
        // construction, before the object is shared). molt's set_add/frozenset
        // hashing authority owns the mutation; gate the type here so a non-set
        // fails closed with SystemError rather than silently no-op'ing.
        let tid = match obj_from_bits(set).as_ptr() {
            Some(ptr) => unsafe { object_type_id(ptr) },
            None => u32::MAX,
        };
        let completed = if tid == TYPE_ID_SET {
            unsafe {
                set_add_in_place(
                    _py,
                    obj_from_bits(set).as_ptr().unwrap(),
                    key,
                    HashContext::SetElement,
                )
            }
        } else if tid == TYPE_ID_FROZENSET {
            crate::object::ops_set::frozenset_add(_py, set, key)
        } else {
            let _ =
                raise_exception::<u64>(_py, BuiltinExc::SystemError, "PySet_Add: expected a set");
            return -1;
        };
        if completed { 0 } else { -1 }
    })
}

/// `PySet_Discard(set, key)` — remove key from set if present. Returns 1 if the
/// key was found and removed, 0 if it was absent, -1 on error. Raises
/// `SystemError` if `set` is not a mutable set and `TypeError` if `key` is
/// unhashable. Never raises `KeyError` (unlike `set.remove`).
pub extern "C" fn PySet_Discard(set: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = match obj_from_bits(set).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_SET => ptr,
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    BuiltinExc::SystemError,
                    "PySet_Discard: expected a set",
                );
                return -1;
            }
        };
        // The deletion result distinguishes absence from callback failure.
        unsafe { set_del_in_place(_py, ptr, key) }
            .map(i32::from)
            .unwrap_or(-1)
    })
}

/// `PySet_Clear(set)` — remove all elements. Returns 0 on success, -1 on error.
pub extern "C" fn PySet_Clear(set: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(set)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET })
        else {
            raise_exception::<()>(py, BuiltinExc::SystemError, "PySet_Clear: expected a set");
            return -1;
        };
        unsafe { set_clear_in_place(py, ptr) };
        0
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Dict Protocol additions
// ---------------------------------------------------------------------------

/// `PyDict_Keys(dict)` — return a list of all keys in the dict.
pub extern "C" fn PyDict_Keys(dict: u64) -> u64 {
    dict_output_as_list(dict, crate::object::ops_dict::DictSnapshotKind::Keys)
}

/// `PyDict_Values(dict)` — return a list of all values in the dict.
pub extern "C" fn PyDict_Values(dict: u64) -> u64 {
    dict_output_as_list(dict, crate::object::ops_dict::DictSnapshotKind::Values)
}

/// `PyDict_Items(dict)` — return a list of all (key, value) pairs in the dict.
pub extern "C" fn PyDict_Items(dict: u64) -> u64 {
    dict_output_as_list(dict, crate::object::ops_dict::DictSnapshotKind::Items)
}

/// `PyDict_Copy(dict)` — return a shallow copy of the dict.
pub extern "C" fn PyDict_Copy(dict: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = c_dict_pointer(_py, dict) else {
            return 0;
        };
        let dict = MoltObject::from_ptr(storage).bits();
        let res = molt_dict_copy(dict);
        if obj_from_bits(res).is_none() { 0 } else { res }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — List Protocol additions
// ---------------------------------------------------------------------------

/// `PyList_Sort(list)` — sort the list in place. Returns 0 on success, -1 on error.
pub extern "C" fn PyList_Sort(list: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::object::ops_list::sort_in_place(py, list, none_bits(), false) {
            0
        } else {
            -1
        }
    })
}

/// `PyList_Reverse(list)` — reverse the list in place. Returns 0 on success, -1 on error.
pub extern "C" fn PyList_Reverse(list: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::object::ops_list::reverse_in_place(py, list) {
            0
        } else {
            -1
        }
    })
}
