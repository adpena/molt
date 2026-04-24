use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::*;

// ─── Memo registry ──────────────────────────────────────────────────────────
// Deep copy requires a memo dict that maps id(original) -> copied_object.
// We use a global handle registry keyed by handle ID so that the Python shim
// can pass memo dicts across multiple intrinsic calls within one deepcopy
// operation.

static MEMO_COUNTER: LazyLock<Mutex<i64>> = LazyLock::new(|| Mutex::new(0));
static MEMO_REGISTRY: LazyLock<Mutex<HashMap<i64, HashMap<u64, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn memo_alloc() -> i64 {
    let mut counter = MEMO_COUNTER.lock().unwrap();
    *counter += 1;
    let id = *counter;
    MEMO_REGISTRY.lock().unwrap().insert(id, HashMap::new());
    id
}

fn memo_drop(handle: i64) {
    MEMO_REGISTRY.lock().unwrap().remove(&handle);
}

fn memo_get(handle: i64, obj_id: u64) -> Option<u64> {
    MEMO_REGISTRY
        .lock()
        .unwrap()
        .get(&handle)
        .and_then(|m| m.get(&obj_id).copied())
}

fn memo_put(handle: i64, obj_id: u64, bits: u64) {
    if let Some(m) = MEMO_REGISTRY.lock().unwrap().get_mut(&handle) {
        m.insert(obj_id, bits);
    }
}

// ─── Type classification ────────────────────────────────────────────────────

fn is_atomic_type_id(type_id: u32) -> bool {
    // Note: None, bool, int, float are NaN-boxed (not heap objects) so they
    // never reach type_id checks — they're handled before as_ptr() returns.
    matches!(
        type_id,
        TYPE_ID_STRING | TYPE_ID_BYTES | TYPE_ID_RANGE | TYPE_ID_NOT_IMPLEMENTED | TYPE_ID_ELLIPSIS
    )
}

// ─── Shallow copy implementation ────────────────────────────────────────────

fn shallow_copy_bits(_py: &PyToken<'_>, bits: u64) -> u64 {
    let obj = obj_from_bits(bits);

    // None and non-ptr immediates (int, float, bool) return self
    if obj.is_none() {
        return bits;
    }
    if obj.as_float().is_some() {
        return bits;
    }
    if to_i64(obj).is_some() {
        return bits;
    }

    let Some(ptr) = obj.as_ptr() else {
        return bits;
    };

    let type_id = unsafe { object_type_id(ptr) };

    // Atomic types return self
    if is_atomic_type_id(type_id) {
        return bits;
    }

    match type_id {
        TYPE_ID_LIST => {
            // Shallow copy: new list with same element refs
            unsafe {
                let src = seq_vec_ref(ptr);
                let new_ptr = alloc_list(_py, src);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(new_ptr).bits()
            }
        }
        TYPE_ID_TUPLE => {
            // Tuples are immutable -- return self
            inc_ref_bits(_py, bits);
            bits
        }
        TYPE_ID_DICT => {
            // Shallow copy: new dict with same key/value refs.
            // Dict order vec stores [k1, v1, k2, v2, ...].
            unsafe {
                let order = dict_order(ptr);
                let new_ptr = alloc_dict_with_pairs(_py, order);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(new_ptr).bits()
            }
        }
        TYPE_ID_SET => {
            // Shallow copy: new set with same element refs
            unsafe {
                let order = set_order(ptr);
                let new_ptr = alloc_set_with_entries(_py, order);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(new_ptr).bits()
            }
        }
        TYPE_ID_FROZENSET => {
            // Frozensets are immutable -- return self
            inc_ref_bits(_py, bits);
            bits
        }
        TYPE_ID_BYTEARRAY => {
            // Copy bytearray data
            unsafe {
                let src = bytearray_vec_ref(ptr);
                let new_ptr = alloc_bytearray(_py, src);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(new_ptr).bits()
            }
        }
        _ => {
            // For other types, check for __copy__ method, otherwise return self.
            // The Python shim handles the full dispatch protocol.
            inc_ref_bits(_py, bits);
            bits
        }
    }
}

// ─── Deep copy implementation ───────────────────────────────────────────────

fn deep_copy_bits(_py: &PyToken<'_>, bits: u64, memo_handle: i64) -> u64 {
    let obj = obj_from_bits(bits);

    // None and non-ptr immediates return self
    if obj.is_none() {
        return bits;
    }
    if obj.as_float().is_some() {
        return bits;
    }
    if to_i64(obj).is_some() {
        return bits;
    }

    // Check memo for already-copied objects (cycle breaking)
    let obj_id = bits;
    if let Some(cached) = memo_get(memo_handle, obj_id) {
        inc_ref_bits(_py, cached);
        return cached;
    }

    let Some(ptr) = obj.as_ptr() else {
        return bits;
    };

    let type_id = unsafe { object_type_id(ptr) };

    // Atomic types return self
    if is_atomic_type_id(type_id) {
        return bits;
    }

    match type_id {
        TYPE_ID_LIST => {
            // Deep copy: recursively copy each element.
            // Allocate empty list first, register in memo to break cycles.
            let new_ptr = alloc_list(_py, &[]);
            if new_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            memo_put(memo_handle, obj_id, new_bits);

            unsafe {
                let src = seq_vec_ref(ptr);
                let len = src.len();
                let new_vec_ptr = *(new_ptr as *mut *mut Vec<u64>);
                let new_vec = &mut *new_vec_ptr;
                new_vec.reserve(len);
                for &elem in src.iter().take(len) {
                    let copied = deep_copy_bits(_py, elem, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    new_vec.push(copied);
                    inc_ref_bits(_py, copied);
                }
            }
            new_bits
        }
        TYPE_ID_TUPLE => {
            // Deep copy tuple: copy all elements, but if all are identical, return self.
            unsafe {
                let src = seq_vec_ref(ptr);
                if src.is_empty() {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                let mut all_same = true;
                let mut copied_elems = Vec::with_capacity(src.len());
                for &elem in src.iter() {
                    let copied = deep_copy_bits(_py, elem, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if copied != elem {
                        all_same = false;
                    }
                    copied_elems.push(copied);
                }
                if all_same {
                    inc_ref_bits(_py, bits);
                    memo_put(memo_handle, obj_id, bits);
                    return bits;
                }
                let new_ptr = alloc_tuple(_py, &copied_elems);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let new_bits = MoltObject::from_ptr(new_ptr).bits();
                memo_put(memo_handle, obj_id, new_bits);
                new_bits
            }
        }
        TYPE_ID_DICT => {
            // Deep copy dict: recursively copy keys and values.
            // Allocate empty dict first for cycle breaking.
            let new_ptr = alloc_dict_with_pairs(_py, &[]);
            if new_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            memo_put(memo_handle, obj_id, new_bits);

            unsafe {
                let order = dict_order(ptr);
                // order is [k1, v1, k2, v2, ...]
                let mut i = 0;
                while i + 1 < order.len() {
                    let key_bits = order[i];
                    let val_bits = order[i + 1];
                    let copied_key = deep_copy_bits(_py, key_bits, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let copied_val = deep_copy_bits(_py, val_bits, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    dict_set_in_place(_py, new_ptr, copied_key, copied_val);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    i += 2;
                }
            }
            new_bits
        }
        TYPE_ID_SET => {
            // Deep copy set
            unsafe {
                let order = set_order(ptr);
                let mut copied_elems = Vec::with_capacity(order.len());
                for &elem in order.iter() {
                    let copied = deep_copy_bits(_py, elem, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    copied_elems.push(copied);
                }
                let new_ptr = alloc_set_with_entries(_py, &copied_elems);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let new_bits = MoltObject::from_ptr(new_ptr).bits();
                memo_put(memo_handle, obj_id, new_bits);
                new_bits
            }
        }
        TYPE_ID_FROZENSET => {
            // Frozensets: deep copy elements, but since frozenset is immutable,
            // if all elements are identical return self.
            unsafe {
                let order = set_order(ptr);
                if order.is_empty() {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                let mut all_same = true;
                let mut copied_elems = Vec::with_capacity(order.len());
                for &elem in order.iter() {
                    let copied = deep_copy_bits(_py, elem, memo_handle);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if copied != elem {
                        all_same = false;
                    }
                    copied_elems.push(copied);
                }
                if all_same {
                    inc_ref_bits(_py, bits);
                    memo_put(memo_handle, obj_id, bits);
                    return bits;
                }
                let new_ptr = alloc_set_like_with_entries(_py, &copied_elems, TYPE_ID_FROZENSET);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let new_bits = MoltObject::from_ptr(new_ptr).bits();
                memo_put(memo_handle, obj_id, new_bits);
                new_bits
            }
        }
        TYPE_ID_BYTEARRAY => {
            // Deep copy bytearray: copy data
            unsafe {
                let src = bytearray_vec_ref(ptr);
                let new_ptr = alloc_bytearray(_py, src);
                if new_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let new_bits = MoltObject::from_ptr(new_ptr).bits();
                memo_put(memo_handle, obj_id, new_bits);
                new_bits
            }
        }
        _ => {
            // For other object types, fall back to returning self.
            // The Python shim handles __deepcopy__, __reduce_ex__, etc.
            inc_ref_bits(_py, bits);
            bits
        }
    }
}

// ─── public intrinsics ──────────────────────────────────────────────────────

/// Shallow copy of a Python object.
/// For container types (list, dict, set), creates a new container with the
/// same element references. For immutable/atomic types, returns the same object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_copy(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { shallow_copy_bits(_py, obj_bits) })
}

/// Deep copy of a Python object with a memo dictionary.
/// `memo_bits` should be a handle obtained from `molt_copy_memo_new`, or
/// None/0 for a fresh memo. Recursively copies all contained objects and
/// handles cycles via the memo registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_deepcopy(obj_bits: u64, memo_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let memo_obj = obj_from_bits(memo_bits);
        let (memo_handle, owned) = if memo_obj.is_none() {
            (memo_alloc(), true)
        } else if let Some(i) = to_i64(memo_obj) {
            (i, false)
        } else {
            (memo_alloc(), true)
        };

        let result = deep_copy_bits(_py, obj_bits, memo_handle);

        if owned {
            memo_drop(memo_handle);
        }

        result
    })
}

/// Allocate a new memo handle for deep copy operations.
/// Returns an integer handle that can be passed to `molt_copy_deepcopy`
/// and later freed with `molt_copy_memo_drop`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_memo_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = memo_alloc();
        MoltObject::from_int(handle).bits()
    })
}

/// Free a memo handle previously allocated with `molt_copy_memo_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_memo_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(handle_bits);
        if let Some(i) = to_i64(obj) {
            memo_drop(i);
        }
        MoltObject::none().bits()
    })
}

/// Raise a `copy.Error` exception with the given message.
#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_error(msg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(msg_bits);
        let msg = string_obj_to_owned(obj).unwrap_or_else(|| "copy.Error".to_string());
        raise_exception::<u64>(_py, "copy.Error", &msg)
    })
}
