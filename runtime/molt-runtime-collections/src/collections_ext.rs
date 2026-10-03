#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime-collections/src/collections_ext.rs ===
//
// Intrinsic implementations for collections.OrderedDict and collections.ChainMap.
//
// Handle model: global Mutex<HashMap<i64, State>> keyed by an atomically-issued
// handle ID, returned to Python as a NaN-boxed integer. Uses a global registry
// (not thread-local) so handles are visible across all threads — critical for
// collections used cross-thread (e.g. deque in queue.Queue, concurrent.futures).
// The GIL serializes all Python-level access, so the Mutex is always uncontended.
//
// dict_order_clone() is a flattened Vec<u64> of [key0, val0, key1, val1, ...] that
// is the canonical ordered representation of a Molt dict object.

use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;

use crate::bridge::{
    ExceptionSentinel, alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, call_callable0, dec_ref_bits,
    dict_del_in_place, dict_get_in_place, dict_like_bits_from_ptr, dict_order_clone,
    dict_set_in_place, exception_pending, inc_ref_bits,
    index_i64_with_overflow, is_truthy, compare_eq, object_type_id, raise_exception,
    raise_key_error_with_key, seq_snapshot, string_obj_to_owned, to_i64, type_name,
};

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

// ─── Handle counters ──────────────────────────────────────────────────────────

fn next_ordereddict_handle() -> i64 {
    collections_state()
        .next_ordereddict_handle
        .fetch_add(1, Ordering::Relaxed)
}

fn next_chainmap_handle() -> i64 {
    collections_state()
        .next_chainmap_handle
        .fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// OrderedDict state
//
// Maintains insertion order via a `Vec<(u64, u64)>` (key_bits, value_bits)
// and a HashMap for O(1) key → vec-index lookup.
//
// Invariant: order.len() == index.len().  The index stores the *current*
// position of each key in `order`.  On deletion we swap-remove and repair.
// ─────────────────────────────────────────────────────────────────────────────

struct OrderedDictState {
    /// Insertion-ordered list of (key_bits, val_bits).
    order: Vec<(u64, u64)>,
    /// Maps a key's hash-tagged u64 (key_bits) to its index in `order`.
    index: HashMap<u64, usize>,
}

impl OrderedDictState {
    fn new() -> Self {
        Self {
            order: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Insert or overwrite `key_bits → value_bits`. Returns old value bits if
    /// the key already existed.
    fn insert(&mut self, key_bits: u64, value_bits: u64) -> Option<u64> {
        if let Some(&idx) = self.index.get(&key_bits) {
            let old = self.order[idx].1;
            self.order[idx].1 = value_bits;
            Some(old)
        } else {
            let idx = self.order.len();
            self.order.push((key_bits, value_bits));
            self.index.insert(key_bits, idx);
            None
        }
    }

    fn get(&self, key_bits: u64) -> Option<u64> {
        self.index.get(&key_bits).map(|&idx| self.order[idx].1)
    }

    fn contains(&self, key_bits: u64) -> bool {
        self.index.contains_key(&key_bits)
    }

    /// Remove key, returning (key_bits, val_bits). Uses swap-remove for O(1),
    /// repairing the displaced entry's index entry.
    fn remove(&mut self, key_bits: u64) -> Option<(u64, u64)> {
        let &idx = self.index.get(&key_bits)?;
        self.index.remove(&key_bits);
        let last_idx = self.order.len() - 1;
        if idx != last_idx {
            self.order.swap(idx, last_idx);
            let displaced_key = self.order[idx].0;
            self.index.insert(displaced_key, idx);
        }
        Some(self.order.pop().unwrap())
    }

    /// Move key to end (last=true) or front (last=false).
    fn move_to_end(&mut self, key_bits: u64, last: bool) {
        let Some(&idx) = self.index.get(&key_bits) else {
            return;
        };
        let entry = self.order.remove(idx);
        // Repair indices for all entries after the removed position.
        for i in idx..self.order.len() {
            let k = self.order[i].0;
            self.index.insert(k, i);
        }
        if last {
            let new_idx = self.order.len();
            self.order.push(entry);
            self.index.insert(key_bits, new_idx);
        } else {
            self.order.insert(0, entry);
            // All existing entries shifted +1.
            for i in 1..=self.order.len() - 1 {
                let k = self.order[i].0;
                self.index.insert(k, i);
            }
            self.index.insert(key_bits, 0);
        }
    }

    /// Pop last (last=true) or first (last=false).
    fn popitem(&mut self, last: bool) -> Option<(u64, u64)> {
        if self.order.is_empty() {
            return None;
        }
        if last {
            let (k, v) = self.order.pop().unwrap();
            self.index.remove(&k);
            Some((k, v))
        } else {
            let (k, v) = self.order.remove(0);
            self.index.remove(&k);
            // Repair all indices shifted -1.
            for i in 0..self.order.len() {
                let ek = self.order[i].0;
                self.index.insert(ek, i);
            }
            Some((k, v))
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn clear(&mut self) {
        self.order.clear();
        self.index.clear();
    }

    fn clone_state(&self) -> Self {
        Self {
            order: self.order.clone(),
            index: self.index.clone(),
        }
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn od_handle_from_bits(_py: &CoreGilToken, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "OrderedDict handle must be an int");
        return None;
    };
    Some(id)
}

// ─── Public intrinsics: OrderedDict ──────────────────────────────────────────

/// Create a new empty OrderedDict. Returns an integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_new() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let id = next_ordereddict_handle();
        collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .insert(id, OrderedDictState::new());
        MoltObject::from_int(id).bits()
    })
}

/// Create an OrderedDict from a list of (k, v) 2-tuples. Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_from_pairs(pairs_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let obj = obj_from_bits(pairs_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected a list of pairs");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "expected a list of pairs");
        }
        let elems = unsafe { seq_snapshot(ptr) };
        let mut state = OrderedDictState::new();
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let Some(elem_ptr) = elem_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
            };
            let elem_type = unsafe { object_type_id(elem_ptr) };
            if elem_type != TYPE_ID_TUPLE && elem_type != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
            }
            let pair = unsafe { seq_snapshot(elem_ptr) };
            if pair.len() < 2 {
                return raise_exception::<_>(_py, "ValueError", "each pair must have 2 elements");
            }
            state.insert(pair[0], pair[1]);
        }
        let id = next_ordereddict_handle();
        collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .insert(id, state);
        MoltObject::from_int(id).bits()
    })
}

/// Set key → value. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_setitem(
    handle_bits: u64,
    key_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = collections_state().ordereddict_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.insert(key_bits, value_bits);
            }
        }
        MoltObject::none().bits()
    })
}

/// Get value for key. Raises KeyError if missing.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_getitem(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let val = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.get(key_bits));
        match val {
            Some(v) => v,
            None => raise_exception::<_>(_py, "KeyError", "key not found"),
        }
    })
}

/// Delete key. Raises KeyError if missing. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_delitem(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.remove(key_bits));
        if removed.is_none() {
            return raise_exception::<_>(_py, "KeyError", "key not found");
        }
        MoltObject::none().bits()
    })
}

/// Return True if key is present.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_contains(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let found = {
            collections_state()
                .ordereddict_registry
                .lock()
                .unwrap()
                .get(&id)
                .map(|s| s.contains(key_bits))
                .unwrap_or(false)
        };
        MoltObject::from_bool(found).bits()
    })
}

/// Return number of entries.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let len = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.len())
            .unwrap_or(0);
        MoltObject::from_int(len as i64).bits()
    })
}

/// Return list of keys in insertion order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_keys(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let keys: Vec<u64> = {
            collections_state()
                .ordereddict_registry
                .lock()
                .unwrap()
                .get(&id)
                .map(|s| s.order.iter().map(|(k, _)| *k).collect())
                .unwrap_or_default()
        };
        let ptr = alloc_list(_py, &keys);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return list of values in insertion order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_values(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let vals: Vec<u64> = {
            collections_state()
                .ordereddict_registry
                .lock()
                .unwrap()
                .get(&id)
                .map(|s| s.order.iter().map(|(_, v)| *v).collect())
                .unwrap_or_default()
        };
        let ptr = alloc_list(_py, &vals);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return list of (key, value) 2-tuples in insertion order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_items(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let pairs: Vec<(u64, u64)> = {
            collections_state()
                .ordereddict_registry
                .lock()
                .unwrap()
                .get(&id)
                .map(|s| s.order.clone())
                .unwrap_or_default()
        };
        let mut tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let tptr = alloc_tuple(_py, &[k, v]);
            if tptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate tuple");
            }
            tuple_bits.push(MoltObject::from_ptr(tptr).bits());
        }
        let lptr = alloc_list(_py, &tuple_bits);
        if lptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(lptr).bits()
    })
}

/// Move key to end (last=True) or front (last=False). Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_move_to_end(
    handle_bits: u64,
    key_bits: u64,
    last_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let last = is_truthy(_py, obj_from_bits(last_bits));
        let found = {
            let mut map = collections_state().ordereddict_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if state.contains(key_bits) {
                    state.move_to_end(key_bits, last);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<_>(_py, "KeyError", "key not found");
        }
        MoltObject::none().bits()
    })
}

/// Remove and return (key, value) from end (last=True) or front (last=False).
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_popitem(handle_bits: u64, last_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let last = is_truthy(_py, obj_from_bits(last_bits));
        let item = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.popitem(last));
        let Some((k, v)) = item else {
            return raise_exception::<_>(_py, "KeyError", "dictionary is empty");
        };
        let tptr = alloc_tuple(_py, &[k, v]);
        if tptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate tuple");
        }
        MoltObject::from_ptr(tptr).bits()
    })
}

/// Pop key, returning value or default. Returns None sentinel if key missing
/// and no default provided (default_bits must be None sentinel or a value).
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_pop(handle_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.remove(key_bits));
        match removed {
            Some((_, v)) => v,
            None => {
                // If caller passed MISSING sentinel (use None for "no default"), raise.
                // Convention: pass None as default_bits to signal "no default provided".
                let default_obj = obj_from_bits(default_bits);
                if default_obj.is_none() {
                    // Distinguish "caller explicitly passed None" from "no default":
                    // The Python wrapper handles this; at intrinsic level we just return
                    // None and let the wrapper detect the KeyError path via the missing
                    // sentinel they pass.
                    raise_exception::<_>(_py, "KeyError", "key not found")
                } else {
                    default_bits
                }
            }
        }
    })
}

/// Update from another OrderedDict handle or a regular dict object. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_update(handle_bits: u64, other_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let other_obj = obj_from_bits(other_bits);
        // Accept either a regular Molt dict or another OrderedDict handle (int).
        if let Some(other_id) = to_i64(other_obj) {
            // Treat as another OrderedDict handle.
            let pairs: Vec<(u64, u64)> = {
                collections_state()
                    .ordereddict_registry
                    .lock()
                    .unwrap()
                    .get(&other_id)
                    .map(|s| s.order.clone())
                    .unwrap_or_default()
            };
            {
                let mut map = collections_state().ordereddict_registry.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for (k, v) in pairs {
                        state.insert(k, v);
                    }
                }
            }
        } else if let Some(ptr) = other_obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_DICT {
                let pairs = unsafe { dict_order_clone(_py, ptr) };
                // pairs is flattened [k0, v0, k1, v1, ...]
                let kv_pairs: Vec<(u64, u64)> =
                    pairs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                {
                    let mut map = collections_state().ordereddict_registry.lock().unwrap();
                    if let Some(state) = map.get_mut(&id) {
                        for (k, v) in kv_pairs {
                            state.insert(k, v);
                        }
                    }
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "update requires a dict");
            }
        } else {
            return raise_exception::<_>(_py, "TypeError", "update requires a dict");
        }
        MoltObject::none().bits()
    })
}

/// Clear all entries. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_clear(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = collections_state().ordereddict_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.clear();
            }
        }
        MoltObject::none().bits()
    })
}

/// Return a shallow copy as a new handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_copy(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let cloned = collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.clone_state());
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid OrderedDict handle");
        };
        let new_id = next_ordereddict_handle();
        collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .insert(new_id, new_state);
        MoltObject::from_int(new_id).bits()
    })
}

/// Release handle resources. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        collections_state()
            .ordereddict_registry
            .lock()
            .unwrap()
            .remove(&id);
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// ChainMap state
//
// A ChainMap is an ordered list of Molt dict object-bits (each must remain
// alive for as long as the ChainMap lives). We store the dict_bits (NaN-boxed
// u64) directly.  The "first" map is maps[0]; updates/deletes operate only on
// maps[0].
// ─────────────────────────────────────────────────────────────────────────────

struct ChainMapState {
    /// List of dict object bits, index 0 is the primary (first) map.
    maps: Vec<u64>,
}

fn cm_handle_from_bits(_py: &CoreGilToken, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "ChainMap handle must be an int");
        return None;
    };
    Some(id)
}

/// Build a new ChainMap from a list of Molt dict objects. Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_new(maps_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let obj = obj_from_bits(maps_bits);
        let mut map_list: Vec<u64> = Vec::new();
        if let Some(ptr) = obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = unsafe { seq_snapshot(ptr) };
                for &elem_bits in elems.iter() {
                    let ep = obj_from_bits(elem_bits);
                    if ep
                        .as_ptr()
                        .is_some_and(|eptr| unsafe { object_type_id(eptr) } != TYPE_ID_DICT)
                    {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "ChainMap maps must be dicts",
                        );
                    }
                    map_list.push(elem_bits);
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "ChainMap expects a list of dicts");
            }
        }
        // If maps_bits is None, start with empty primary dict.
        if obj.is_none() {
            let empty_ptr = alloc_dict_with_pairs(_py, &[]);
            if empty_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate dict");
            }
            map_list.push(MoltObject::from_ptr(empty_ptr).bits());
        }
        // Ensure there is always at least one map.
        if map_list.is_empty() {
            let empty_ptr = alloc_dict_with_pairs(_py, &[]);
            if empty_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate dict");
            }
            map_list.push(MoltObject::from_ptr(empty_ptr).bits());
        }
        let id = next_chainmap_handle();
        collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .insert(id, ChainMapState { maps: map_list });
        MoltObject::from_int(id).bits()
    })
}

/// Search all maps in order for key. Returns value or raises KeyError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_getitem(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        for dict_bits in maps {
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                continue;
            }
            if let Some(val) = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
                return val;
            }
        }
        raise_exception::<_>(_py, "KeyError", "key not found")
    })
}

/// Set key in the first (primary) map. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_setitem(handle_bits: u64, key_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let first_map_bits = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.maps.first().copied());
        let Some(dict_bits) = first_map_bits else {
            return raise_exception::<_>(_py, "KeyError", "ChainMap has no maps");
        };
        let dict_obj = obj_from_bits(dict_bits);
        let Some(dict_ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "first map is not a dict");
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "first map is not a dict");
        }
        unsafe {
            dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
        }
        MoltObject::none().bits()
    })
}

/// Delete key from the first map only. Raises KeyError if not found there.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_delitem(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let first_map_bits = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.maps.first().copied());
        let Some(dict_bits) = first_map_bits else {
            return raise_exception::<_>(_py, "KeyError", "key not found");
        };
        let dict_obj = obj_from_bits(dict_bits);
        let Some(dict_ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "KeyError", "key not found");
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "KeyError", "key not found");
        }
        let deleted = unsafe { dict_del_in_place(_py, dict_ptr, key_bits) };
        if !deleted {
            return raise_exception::<_>(_py, "KeyError", "key not found in first map");
        }
        MoltObject::none().bits()
    })
}

/// Return True if key exists in any map.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_contains(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        for dict_bits in maps {
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                continue;
            }
            if unsafe { dict_get_in_place(_py, dict_ptr, key_bits) }.is_some() {
                return MoltObject::from_bool(true).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

/// Return total count of unique keys across all maps.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        // Collect unique key bits across all maps.
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for dict_bits in maps {
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                continue;
            }
            let order = unsafe { dict_order_clone(_py, dict_ptr) };
            let mut i = 0;
            while i + 1 < order.len() {
                seen.insert(order[i]);
                i += 2;
            }
        }
        MoltObject::from_int(seen.len() as i64).bits()
    })
}

/// Return list of unique keys (first occurrence wins, preserving dict order).
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_keys(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut keys: Vec<u64> = Vec::new();
        for dict_bits in maps {
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                continue;
            }
            let order = unsafe { dict_order_clone(_py, dict_ptr) };
            let mut i = 0;
            while i + 1 < order.len() {
                let k = order[i];
                if seen.insert(k) {
                    keys.push(k);
                }
                i += 2;
            }
        }
        let ptr = alloc_list(_py, &keys);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return a new ChainMap with an optional new map prepended. Returns handle.
/// If m_bits is None, prepend a fresh empty dict.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_new_child(handle_bits: u64, m_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let existing_maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        let new_first = {
            let m_obj = obj_from_bits(m_bits);
            if m_obj.is_none() {
                let empty_ptr = alloc_dict_with_pairs(_py, &[]);
                if empty_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate dict");
                }
                MoltObject::from_ptr(empty_ptr).bits()
            } else {
                if m_obj
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) } != TYPE_ID_DICT)
                {
                    return raise_exception::<_>(_py, "TypeError", "new_child map must be a dict");
                }
                m_bits
            }
        };
        let mut new_maps = Vec::with_capacity(existing_maps.len() + 1);
        new_maps.push(new_first);
        new_maps.extend_from_slice(&existing_maps);
        let new_id = next_chainmap_handle();
        collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .insert(new_id, ChainMapState { maps: new_maps });
        MoltObject::from_int(new_id).bits()
    })
}

/// Return a new ChainMap containing all maps except the first. Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_parents(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let mut maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        if !maps.is_empty() {
            maps.remove(0);
        }
        if maps.is_empty() {
            let empty_ptr = alloc_dict_with_pairs(_py, &[]);
            if empty_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate dict");
            }
            maps.push(MoltObject::from_ptr(empty_ptr).bits());
        }
        let new_id = next_chainmap_handle();
        collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .insert(new_id, ChainMapState { maps });
        MoltObject::from_int(new_id).bits()
    })
}

/// Return list of underlying dict objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_maps(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.maps.clone())
            .unwrap_or_default();
        let ptr = alloc_list(_py, &maps);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Release ChainMap handle. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        collections_state()
            .chainmap_registry
            .lock()
            .unwrap()
            .remove(&id);
        MoltObject::none().bits()
    })
}
// ─── Handle counter ─────────────────────────────────────────────────────────

fn next_deque_handle() -> i64 {
    collections_state()
        .next_deque_handle
        .fetch_add(1, Ordering::Relaxed)
}

// ─── Deque state ────────────────────────────────────────────────────────────

struct DequeState {
    data: VecDeque<u64>,
    maxlen: Option<usize>,
    mutation_version: u64,
}

impl DequeState {
    fn new(maxlen: Option<usize>) -> Self {
        Self {
            data: VecDeque::new(),
            maxlen,
            mutation_version: 0,
        }
    }

    fn from_iterable_elements(_py: &CoreGilToken, elems: &[u64], maxlen: Option<usize>) -> Self {
        let start = maxlen
            .filter(|&ml| elems.len() > ml)
            .map(|ml| elems.len() - ml)
            .unwrap_or(0);
        let mut data = VecDeque::with_capacity(elems.len().saturating_sub(start));
        for &bits in &elems[start..] {
            data.push_back(retain_handle_value(_py, bits));
        }
        Self {
            data,
            maxlen,
            mutation_version: 0,
        }
    }

    fn retain_snapshot(&self, _py: &CoreGilToken) -> Vec<u64> {
        let mut snapshot = Vec::with_capacity(self.data.len());
        for &bits in &self.data {
            snapshot.push(retain_handle_value(_py, bits));
        }
        snapshot
    }

    fn retain_snapshot_with_version(&self, _py: &CoreGilToken) -> (Vec<u64>, u64) {
        (self.retain_snapshot(_py), self.mutation_version)
    }

    fn mark_size_or_order_mutation(&mut self) {
        self.mutation_version = self.mutation_version.wrapping_add(1);
    }

    fn clone_state(&self, _py: &CoreGilToken) -> Self {
        Self {
            data: self.retain_snapshot(_py).into(),
            maxlen: self.maxlen,
            mutation_version: 0,
        }
    }

    fn release_all(&mut self, _py: &CoreGilToken) {
        while let Some(bits) = self.data.pop_front() {
            release_handle_value(_py, bits);
        }
    }
}

struct RetainedDequeSnapshot<'py> {
    py: &'py CoreGilToken,
    items: Vec<u64>,
}

impl<'py> RetainedDequeSnapshot<'py> {
    fn new(py: &'py CoreGilToken, items: Vec<u64>) -> Self {
        Self { py, items }
    }

    fn as_slice(&self) -> &[u64] {
        &self.items
    }
}

impl Drop for RetainedDequeSnapshot<'_> {
    fn drop(&mut self) {
        for bits in self.items.drain(..) {
            release_handle_value(self.py, bits);
        }
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn deque_handle_from_bits(_py: &CoreGilToken, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "deque handle must be an int");
        return None;
    };
    Some(id)
}

fn retained_deque_snapshot_with_version(_py: &CoreGilToken, id: i64) -> (Vec<u64>, u64) {
    collections_state()
        .deque_registry
        .lock()
        .unwrap()
        .get(&id)
        .map(|s| s.retain_snapshot_with_version(_py))
        .unwrap_or_default()
}

/// Comparison cursors hold no registry guard across a Python callback. Element
/// replacement keeps the structural version but must be observed at the next
/// position, so each item is retained from current storage independently.
fn deque_comparison_start(id: i64) -> (usize, u64) {
    collections_state().deque_registry.lock().unwrap().get(&id)
        .map(|state| (state.data.len(), state.mutation_version)).unwrap_or_default()
}

fn deque_comparison_item(py: &CoreGilToken, id: i64, index: usize) -> Option<u64> {
    collections_state().deque_registry.lock().unwrap().get(&id)
        .and_then(|state| state.data.get(index).copied())
        .map(|bits| retain_handle_value(py, bits))
}
fn deque_mutated_since(id: i64, expected_version: u64) -> bool {
    collections_state()
        .deque_registry
        .lock()
        .unwrap()
        .get(&id)
        .map(|s| s.mutation_version != expected_version)
        .unwrap_or(true)
}

/// Parse maxlen_bits into Option<usize>.
/// Returns Ok(None) for Python None (unbounded), Ok(Some(n)) for non-negative int,
/// or Err(()) after raising ValueError for negative.
fn parse_maxlen(_py: &CoreGilToken, maxlen_bits: u64) -> Result<Option<usize>, ()> {
    let obj = obj_from_bits(maxlen_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(n) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "an integer is required");
        return Err(());
    };
    if n < 0 {
        let _ = raise_exception::<u64>(_py, "ValueError", "maxlen must be non-negative");
        return Err(());
    }
    Ok(Some(n as usize))
}

/// Extract elements from a list or tuple pointer. Returns None and raises
/// TypeError if the bits are not a list or tuple.
fn extract_iterable_elements(
    _py: &CoreGilToken,
    iterable_bits: u64,
) -> Option<OwnedBridgeHandleSnapshot> {
    let obj = obj_from_bits(iterable_bits);
    let Some(ptr) = obj.as_ptr() else {
        let _ = raise_exception::<u64>(_py, "TypeError", "argument must be an iterable");
        return None;
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        let _ = raise_exception::<u64>(_py, "TypeError", "argument must be an iterable");
        return None;
    }
    Some(unsafe { seq_snapshot(ptr) })
}

/// Resolve a potentially negative index against a given length.
/// Returns the resolved index or None if out of bounds.
fn resolve_index(index: i64, len: usize) -> Option<usize> {
    let resolved = if index < 0 { index + len as i64 } else { index };
    if resolved < 0 || resolved >= len as i64 {
        None
    } else {
        Some(resolved as usize)
    }
}

// ─── Public intrinsics: deque ───────────────────────────────────────────────

/// Create a new empty deque with optional maxlen. Returns an integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_new(maxlen_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let maxlen = match parse_maxlen(_py, maxlen_bits) {
            Ok(m) => m,
            Err(()) => return MoltObject::none().bits(),
        };
        let id = next_deque_handle();
        collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .insert(id, DequeState::new(maxlen));
        MoltObject::from_int(id).bits()
    })
}

/// Create a deque from an iterable (list/tuple) with optional maxlen.
/// If maxlen is set and the iterable is longer, keeps only the last maxlen elements.
/// Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_from_iterable(iterable_bits: u64, maxlen_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let maxlen = match parse_maxlen(_py, maxlen_bits) {
            Ok(m) => m,
            Err(()) => return MoltObject::none().bits(),
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        let state = DequeState::from_iterable_elements(_py, elems.as_ref(), maxlen);
        let id = next_deque_handle();
        collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .insert(id, state);
        MoltObject::from_int(id).bits()
    })
}

/// Append item to the right end. If bounded and full, pop from the left.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_append(handle_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let mut evicted = None;
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(ml) = state.maxlen {
                    if ml == 0 {
                        // maxlen is 0, element is silently dropped.
                        return MoltObject::none().bits();
                    }
                    if state.data.len() == ml {
                        evicted = state.data.pop_front();
                    }
                }
                let retained = retain_handle_value(_py, item_bits);
                state.data.push_back(retained);
                state.mark_size_or_order_mutation();
            }
        }
        if let Some(bits) = evicted {
            release_handle_value(_py, bits);
        }
        MoltObject::none().bits()
    })
}

/// Append item to the left end. If bounded and full, pop from the right.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_appendleft(handle_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let mut evicted = None;
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(ml) = state.maxlen {
                    if ml == 0 {
                        return MoltObject::none().bits();
                    }
                    if state.data.len() == ml {
                        evicted = state.data.pop_back();
                    }
                }
                let retained = retain_handle_value(_py, item_bits);
                state.data.push_front(retained);
                state.mark_size_or_order_mutation();
            }
        }
        if let Some(bits) = evicted {
            release_handle_value(_py, bits);
        }
        MoltObject::none().bits()
    })
}

/// Remove and return the rightmost element.
/// Raises IndexError if the deque is empty.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_pop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let result = collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| {
                let popped = s.data.pop_back();
                if popped.is_some() {
                    s.mark_size_or_order_mutation();
                }
                popped
            });
        match result {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "IndexError", "pop from an empty deque"),
        }
    })
}

/// Remove and return the leftmost element.
/// Raises IndexError if the deque is empty.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_popleft(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let result = collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| {
                let popped = s.data.pop_front();
                if popped.is_some() {
                    s.mark_size_or_order_mutation();
                }
                popped
            });
        match result {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "IndexError", "pop from an empty deque"),
        }
    })
}

/// Extend the right side from an iterable (list/tuple).
/// For bounded deques, evicts from the left as needed.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_extend(handle_bits: u64, iterable_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        let mut evicted = Vec::new();
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                let mut mutated = false;
                for &item in elems.iter() {
                    if let Some(ml) = state.maxlen {
                        if ml == 0 {
                            continue;
                        }
                        if state.data.len() == ml
                            && let Some(bits) = state.data.pop_front()
                        {
                            evicted.push(bits);
                        }
                    }
                    state.data.push_back(retain_handle_value(_py, item));
                    mutated = true;
                }
                if mutated {
                    state.mark_size_or_order_mutation();
                }
            }
        }
        for bits in evicted {
            release_handle_value(_py, bits);
        }
        MoltObject::none().bits()
    })
}

/// Extend the left side from an iterable (list/tuple).
/// NOTE: per CPython semantics, this reverses the order of elements from the
/// iterable (equivalent to appendleft() for each element in order).
/// For bounded deques, evicts from the right as needed.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_extendleft(handle_bits: u64, iterable_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        let mut evicted = Vec::new();
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                let mut mutated = false;
                // Each element is prepended in order, which reverses the iterable.
                for &item in elems.iter() {
                    if let Some(ml) = state.maxlen {
                        if ml == 0 {
                            continue;
                        }
                        if state.data.len() == ml
                            && let Some(bits) = state.data.pop_back()
                        {
                            evicted.push(bits);
                        }
                    }
                    state.data.push_front(retain_handle_value(_py, item));
                    mutated = true;
                }
                if mutated {
                    state.mark_size_or_order_mutation();
                }
            }
        }
        for bits in evicted {
            release_handle_value(_py, bits);
        }
        MoltObject::none().bits()
    })
}

/// Rotate the deque n steps.
/// n > 0: rotate right (equivalent to appendleft(pop()) n times).
/// n < 0: rotate left (equivalent to append(popleft()) |n| times).
/// n == 0 or empty deque: no-op.
/// Uses VecDeque::rotate_right/rotate_left for O(min(n, len)) performance.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_rotate(handle_bits: u64, n_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let n_obj = obj_from_bits(n_bits);
        let Some(n) = to_i64(n_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                let len = state.data.len();
                if len > 0 {
                    if n > 0 {
                        let steps = (n as usize) % len;
                        if steps > 0 {
                            state.data.rotate_right(steps);
                        }
                    } else if n < 0 {
                        let steps = ((-n) as usize) % len;
                        if steps > 0 {
                            state.data.rotate_left(steps);
                        }
                    }
                    state.mark_size_or_order_mutation();
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// Return the length of the deque as a NaN-boxed int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let len = collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.data.len())
            .unwrap_or(0);
        MoltObject::from_int(len as i64).bits()
    })
}

/// Get element at index. Supports negative indexing.
/// Raises IndexError "deque index out of range" if out of bounds.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_getitem(handle_bits: u64, index_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let type_err = format!(
            "sequence index must be integer, not '{}'",
            type_name(_py, obj_from_bits(index_bits))
        );
        let Some(index) = index_i64_with_overflow(_py, index_bits, &type_err, None) else {
            return MoltObject::none().bits();
        };
        let result = {
            let map = collections_state().deque_registry.lock().unwrap();
            map.get(&id).and_then(|state| {
                let resolved = resolve_index(index, state.data.len())?;
                state.data.get(resolved).copied()
            })
        };
        match result {
            Some(bits) => retain_handle_value(_py, bits),
            None => raise_exception::<_>(_py, "IndexError", "deque index out of range"),
        }
    })
}

/// Set element at index. Supports negative indexing.
/// Raises IndexError if out of bounds.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_setitem(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let type_err = format!(
            "sequence index must be integer, not '{}'",
            type_name(_py, obj_from_bits(index_bits))
        );
        let Some(index) = index_i64_with_overflow(_py, index_bits, &type_err, None) else {
            return MoltObject::none().bits();
        };
        let replaced = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(resolved) = resolve_index(index, state.data.len()) {
                    let retained = retain_handle_value(_py, value_bits);
                    Some(std::mem::replace(&mut state.data[resolved], retained))
                } else {
                    None
                }
            } else {
                None
            }
        };
        match replaced {
            Some(old) => {
                release_handle_value(_py, old);
                MoltObject::none().bits()
            }
            None => raise_exception::<_>(_py, "IndexError", "deque index out of range"),
        }
    })
}

/// Delete element at index. Supports negative indexing.
/// Uses VecDeque::remove() which is O(min(i, n-i)).
/// Raises IndexError if out of bounds.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_delitem(handle_bits: u64, index_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let type_err = format!(
            "sequence index must be integer, not '{}'",
            type_name(_py, obj_from_bits(index_bits))
        );
        let Some(index) = index_i64_with_overflow(_py, index_bits, &type_err, None) else {
            return MoltObject::none().bits();
        };
        let removed = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(resolved) = resolve_index(index, state.data.len()) {
                    let removed = state.data.remove(resolved);
                    if removed.is_some() {
                        state.mark_size_or_order_mutation();
                    }
                    removed
                } else {
                    None
                }
            } else {
                None
            }
        };
        match removed {
            Some(bits) => {
                release_handle_value(_py, bits);
                MoltObject::none().bits()
            }
            None => raise_exception::<_>(_py, "IndexError", "deque index out of range"),
        }
    })
}

/// Return True if item is found in the deque via rich equality comparison.
/// Uses iterator, no allocation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_contains(handle_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let (len, mutation_version) = deque_comparison_start(id);
        let target = obj_from_bits(item_bits);
        for index in 0..len {
            let Some(elem_bits) = deque_comparison_item(_py, id, index) else {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            };
            let compared = compare_eq(_py, obj_from_bits(elem_bits), target);
            release_handle_value(_py, elem_bits);
            let matched = match compared {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            if matched {
                return MoltObject::from_bool(true).bits();
            }
            if deque_mutated_since(id, mutation_version) {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

/// Count elements equal to item via rich equality. Returns count as NaN-boxed int.
/// Reloads and retains one live element per comparison.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_count(handle_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let (len, mutation_version) = deque_comparison_start(id);
        let target = obj_from_bits(item_bits);
        let mut count: i64 = 0;
        for index in 0..len {
            let Some(elem_bits) = deque_comparison_item(_py, id, index) else {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            };
            let compared = compare_eq(_py, obj_from_bits(elem_bits), target);
            release_handle_value(_py, elem_bits);
            let matched = match compared {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            if deque_mutated_since(id, mutation_version) {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            }
            if matched {
                count += 1;
            }
        }
        MoltObject::from_int(count).bits()
    })
}

/// Search for item in range [start, stop). Negative indices resolved relative
/// to length, clamped to [0, len].
/// Raises ValueError "x is not in deque" if not found.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_index(
    handle_bits: u64,
    item_bits: u64,
    start_bits: u64,
    stop_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let start_obj = obj_from_bits(start_bits);
        let stop_obj = obj_from_bits(stop_bits);
        let Some(start_raw) = to_i64(start_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let Some(stop_raw) = to_i64(stop_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let (len, mutation_version) = deque_comparison_start(id);
        let len = len as i64;
        // Resolve negative indices.
        let mut start = if start_raw < 0 {
            start_raw + len
        } else {
            start_raw
        };
        let mut stop = if stop_raw < 0 {
            stop_raw + len
        } else {
            stop_raw
        };
        // Clamp to [0, len].
        if start < 0 {
            start = 0;
        }
        if start > len {
            start = len;
        }
        if stop < 0 {
            stop = 0;
        }
        if stop > len {
            stop = len;
        }
        let target = obj_from_bits(item_bits);
        let start_usize = start as usize;
        let stop_usize = stop as usize;
        for i in start_usize..stop_usize {
            let Some(elem_bits) = deque_comparison_item(_py, id, i) else {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            };
            let compared = compare_eq(_py, obj_from_bits(elem_bits), target);
            release_handle_value(_py, elem_bits);
            let matched = match compared {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            if matched {
                return MoltObject::from_int(i as i64).bits();
            }
            if deque_mutated_since(id, mutation_version) {
                return raise_exception::<_>(_py, "RuntimeError", "deque mutated during iteration");
            }
        }
        raise_exception::<_>(_py, "ValueError", "x is not in deque")
    })
}

/// Insert item at index position. If bounded and len == maxlen, raises
/// IndexError "deque already at its maximum size".
/// Negative indices resolve but clamp to 0. Index beyond len clamps to len.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_insert(handle_bits: u64, index_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let idx_obj = obj_from_bits(index_bits);
        let Some(index) = to_i64(idx_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let ok = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                // Check maxlen constraint before insertion.
                if let Some(ml) = state.maxlen {
                    if state.data.len() >= ml {
                        Err(())
                    } else {
                        let len = state.data.len() as i64;
                        let mut resolved = if index < 0 { index + len } else { index };
                        if resolved < 0 {
                            resolved = 0;
                        }
                        if resolved > len {
                            resolved = len;
                        }
                        state
                            .data
                            .insert(resolved as usize, retain_handle_value(_py, item_bits));
                        state.mark_size_or_order_mutation();
                        Ok(())
                    }
                } else {
                    let len = state.data.len() as i64;
                    let mut resolved = if index < 0 { index + len } else { index };
                    if resolved < 0 {
                        resolved = 0;
                    }
                    if resolved > len {
                        resolved = len;
                    }
                    state
                        .data
                        .insert(resolved as usize, retain_handle_value(_py, item_bits));
                    state.mark_size_or_order_mutation();
                    Ok(())
                }
            } else {
                Ok(())
            }
        };
        match ok {
            Ok(()) => MoltObject::none().bits(),
            Err(()) => raise_exception::<_>(_py, "IndexError", "deque already at its maximum size"),
        }
    })
}

/// Remove first occurrence of item. Raises ValueError
/// "deque.remove(x): x not in deque" if not found.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_remove(handle_bits: u64, item_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let (len, mutation_version) = deque_comparison_start(id);
        let target = obj_from_bits(item_bits);
        let mut found_idx: Option<usize> = None;
        for i in 0..len {
            let Some(elem_bits) = deque_comparison_item(_py, id, i) else {
                return raise_exception::<_>(_py, "IndexError", "deque mutated during iteration");
            };
            let compared = compare_eq(_py, obj_from_bits(elem_bits), target);
            release_handle_value(_py, elem_bits);
            let matched = match compared {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            if deque_mutated_since(id, mutation_version) {
                return raise_exception::<_>(_py, "IndexError", "deque mutated during iteration");
            }
            if matched {
                found_idx = Some(i);
                break;
            }
        }
        let Some(idx) = found_idx else {
            return raise_exception::<_>(_py, "ValueError", "deque.remove(x): x not in deque");
        };
        let removed = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                let removed = state.data.remove(idx);
                if removed.is_some() {
                    state.mark_size_or_order_mutation();
                }
                removed
            } else {
                None
            }
        }
        .unwrap_or_else(|| MoltObject::none().bits());
        release_handle_value(_py, removed);
        MoltObject::none().bits()
    })
}

/// Reverse the deque in place.
/// Uses make_contiguous() + reverse() for performance.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_reverse(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.data.make_contiguous().reverse();
            }
        }
        MoltObject::none().bits()
    })
}

/// Clear all elements. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_clear(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed: Vec<u64> = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if !state.data.is_empty() {
                    state.mark_size_or_order_mutation();
                }
                state.data.drain(..).collect()
            } else {
                Vec::new()
            }
        };
        for bits in removed {
            release_handle_value(_py, bits);
        }
        MoltObject::none().bits()
    })
}

/// Create a shallow copy. The new deque owns retained references to the same elements.
/// Returns a new handle with the same maxlen.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_copy(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let cloned = collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.clone_state(_py));
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid deque handle");
        };
        let new_id = next_deque_handle();
        collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .insert(new_id, new_state);
        MoltObject::from_int(new_id).bits()
    })
}

/// Return maxlen as NaN-boxed int, or None if unbounded.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_maxlen(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maxlen = collections_state()
            .deque_registry
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.maxlen);
        match maxlen {
            Some(ml) => MoltObject::from_int(ml as i64).bits(),
            None => MoltObject::none().bits(),
        }
    })
}

/// Remove handle from the global registry. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = {
            let mut map = collections_state().deque_registry.lock().unwrap();
            map.remove(&id)
        };
        if let Some(mut state) = removed {
            state.release_all(_py);
        }
        MoltObject::none().bits()
    })
}

// ─── defaultdict state ─────────────────────────────────────────────────────

struct DefaultDictState {
    factory_bits: u64,
}

fn retain_handle_value(_py: &CoreGilToken, bits: u64) -> u64 {
    if obj_from_bits(bits).as_ptr().is_some() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn release_handle_value(_py: &CoreGilToken, bits: u64) {
    if obj_from_bits(bits).as_ptr().is_some() {
        dec_ref_bits(_py, bits);
    }
}

struct CollectionsRuntimeState {
    next_ordereddict_handle: AtomicI64,
    next_chainmap_handle: AtomicI64,
    next_deque_handle: AtomicI64,
    next_defaultdict_handle: AtomicI64,
    ordereddict_registry: Mutex<HashMap<i64, OrderedDictState>>,
    chainmap_registry: Mutex<HashMap<i64, ChainMapState>>,
    deque_registry: Mutex<HashMap<i64, DequeState>>,
    defaultdict_registry: Mutex<HashMap<i64, DefaultDictState>>,
}

impl CollectionsRuntimeState {
    fn new() -> Self {
        Self {
            next_ordereddict_handle: AtomicI64::new(1),
            next_chainmap_handle: AtomicI64::new(1),
            next_deque_handle: AtomicI64::new(1),
            next_defaultdict_handle: AtomicI64::new(1),
            ordereddict_registry: Mutex::new(HashMap::new()),
            chainmap_registry: Mutex::new(HashMap::new()),
            deque_registry: Mutex::new(HashMap::new()),
            defaultdict_registry: Mutex::new(HashMap::new()),
        }
    }

    fn clear(&self, _py: &CoreGilToken) {
        self.ordereddict_registry.lock().unwrap().clear();
        self.chainmap_registry.lock().unwrap().clear();
        let drained_deques: Vec<DequeState> = {
            let mut deques = self.deque_registry.lock().unwrap();
            deques.drain().map(|(_, state)| state).collect()
        };
        for mut state in drained_deques {
            state.release_all(_py);
        }
        {
            let mut defaultdicts = self.defaultdict_registry.lock().unwrap();
            for (_, state) in defaultdicts.drain() {
                release_handle_value(_py, state.factory_bits);
            }
        }
    }
}

unsafe extern "C" fn collections_runtime_state_init() -> *mut u8 {
    Box::into_raw(Box::new(CollectionsRuntimeState::new())) as *mut u8
}

unsafe extern "C" fn collections_runtime_state_clear(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    molt_runtime_core::with_core_gil!(py, unsafe {
        (&*(ptr as *const CollectionsRuntimeState)).clear(py);
    });
}

unsafe extern "C" fn collections_runtime_state_drop(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    molt_runtime_core::with_core_gil!(py, unsafe {
        let state = Box::from_raw(ptr as *mut CollectionsRuntimeState);
        state.clear(py);
        drop(state);
    });
}

fn collections_state() -> &'static CollectionsRuntimeState {
    let ptr = crate::bridge::runtime_state_get_or_init(
        b"molt-runtime-collections/v1",
        collections_runtime_state_init,
        collections_runtime_state_clear,
        collections_runtime_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt collections runtime state initialization failed"
    );
    unsafe { &*(ptr as *const CollectionsRuntimeState) }
}

// ─── Handle counters ────────────────────────────────────────────────────────

fn next_defaultdict_handle() -> i64 {
    collections_state()
        .next_defaultdict_handle
        .fetch_add(1, Ordering::Relaxed)
}

// ─── Handle helpers ─────────────────────────────────────────────────────────

fn dd_handle_from_bits(_py: &CoreGilToken, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "defaultdict handle must be an int");
        return None;
    };
    Some(id)
}

fn defaultdict_missing_value(_py: &CoreGilToken, handle_bits: u64, key_bits: u64) -> u64 {
    let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
        return MoltObject::none().bits();
    };
    let factory = collections_state()
        .defaultdict_registry
        .lock()
        .unwrap()
        .get(&id)
        .map(|s| s.factory_bits);
    let Some(factory_bits) = factory else {
        return raise_exception::<_>(_py, "RuntimeError", "invalid defaultdict handle");
    };
    let factory_obj = obj_from_bits(factory_bits);
    if factory_obj.is_none() {
        return raise_key_error_with_key::<u64>(_py, key_bits);
    }
    let val = call_callable0(_py, factory_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    val
}

// ─── defaultdict intrinsics ─────────────────────────────────────────────────

/// Create a new defaultdict handle storing the factory callable (or None).
/// Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_new(factory_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let id = next_defaultdict_handle();
        let factory_bits = retain_handle_value(_py, factory_bits);
        collections_state()
            .defaultdict_registry
            .lock()
            .unwrap()
            .insert(id, DefaultDictState { factory_bits });
        MoltObject::from_int(id).bits()
    })
}

/// Called when __getitem__ doesn't find a key.
/// If factory is None, raise KeyError.
/// If factory is not None, call it with 0 args and return the default value
/// bits.  The Python caller is responsible for inserting the value into the
/// underlying dict.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_missing(handle_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        defaultdict_missing_value(_py, handle_bits, key_bits)
    })
}

/// Native `defaultdict.__missing__(self, key)` method.
/// Reads the intrinsic state handle from the instance slot, computes the
/// default value or raises KeyError, and inserts factory-produced values into
/// the dict-subclass backing store before returning them.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_missing_method(self_bits: u64, key_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "defaultdict.__missing__ expects self");
        };
        let Some(handle_name_bits) = attr_name_bits_from_bytes(_py, b"_dd_handle") else {
            return MoltObject::none().bits();
        };
        let handle_bits = unsafe { attr_lookup_ptr_allow_missing(_py, self_ptr, handle_name_bits) };
        dec_ref_bits(_py, handle_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(handle_bits) = handle_bits else {
            return raise_exception::<_>(_py, "RuntimeError", "defaultdict handle is missing");
        };
        let val = defaultdict_missing_value(_py, handle_bits, key_bits);
        dec_ref_bits(_py, handle_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(dict_bits) = (unsafe { dict_like_bits_from_ptr(_py, self_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "defaultdict storage is unavailable");
        };
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "defaultdict storage is unavailable");
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "defaultdict storage is unavailable",
                );
            }
            dict_set_in_place(_py, dict_ptr, key_bits, val);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, val);
            return MoltObject::none().bits();
        }
        val
    })
}

/// Return the factory_bits.  If None, return None bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_factory(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let factory_bits = collections_state()
            .defaultdict_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.factory_bits)
            .unwrap_or_else(|| MoltObject::none().bits());
        retain_handle_value(_py, factory_bits)
    })
}

/// Create new handle with the same factory_bits.  Returns new handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_copy(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let factory = collections_state()
            .defaultdict_registry
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.factory_bits);
        let Some(factory_bits) = factory else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid defaultdict handle");
        };
        let new_id = next_defaultdict_handle();
        let factory_bits = retain_handle_value(_py, factory_bits);
        collections_state()
            .defaultdict_registry
            .lock()
            .unwrap()
            .insert(new_id, DefaultDictState { factory_bits });
        MoltObject::from_int(new_id).bits()
    })
}

/// Release defaultdict handle.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = collections_state()
            .defaultdict_registry
            .lock()
            .unwrap()
            .remove(&id);
        if let Some(state) = removed {
            release_handle_value(_py, state.factory_bits);
        }
        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// namedtuple field validation intrinsic
// ─────────────────────────────────────────────────────────────────────────────

fn is_python_keyword(name: &str) -> bool {
    matches!(
        name,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// `molt_namedtuple_validate_fields(typename, field_names_list, rename)`
///
/// Validates and normalizes namedtuple field names. Returns a tuple of
/// (normalized_field_names_tuple,) on success, or raises ValueError/TypeError.
///
/// - typename_bits: str — the typename to validate
/// - fields_bits: list[str] — the field names (already split from string)
/// - rename_bits: bool — whether to auto-rename invalid names
///
/// Returns: tuple[str, ...] of validated/normalized field names.
#[unsafe(no_mangle)]
pub extern "C" fn molt_namedtuple_validate_fields(
    typename_bits: u64,
    fields_bits: u64,
    rename_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        // Validate typename
        let Some(typename) = string_obj_to_owned(obj_from_bits(typename_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "typename must be a string");
        };
        if !is_valid_identifier(&typename) || is_python_keyword(&typename) {
            let msg = format!("Type names and field names must be valid identifiers: {typename:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }

        // Get rename flag
        let rename = is_truthy(_py, obj_from_bits(rename_bits));

        // Get field names list
        let Some(fields_ptr) = obj_from_bits(fields_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "field_names must be a sequence");
        };
        let field_type = unsafe { object_type_id(fields_ptr) };
        if field_type != TYPE_ID_LIST && field_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "field_names must be a sequence");
        }
        let elems = unsafe { seq_snapshot(fields_ptr) };

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut normalized: Vec<u64> = Vec::new();

        for (idx, &elem_bits) in elems.iter().enumerate() {
            let Some(name) = string_obj_to_owned(obj_from_bits(elem_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "field names must be strings");
            };

            let invalid = !is_valid_identifier(&name)
                || is_python_keyword(&name)
                || name.starts_with('_')
                || seen.contains(&name);

            let final_name = if invalid {
                if rename {
                    format!("_{idx}")
                } else {
                    if seen.contains(&name) {
                        let msg = format!("Encountered duplicate field name: {name:?}");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    if name.starts_with('_') {
                        let msg = format!("Field names cannot start with an underscore: {name:?}");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    let msg =
                        format!("Type names and field names must be valid identifiers: {name:?}");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
            } else {
                name
            };

            // After rename, check for duplicates again
            if seen.contains(&final_name) {
                let msg = format!("Encountered duplicate field name: {final_name:?}");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            seen.insert(final_name.clone());

            let name_ptr = alloc_string(_py, final_name.as_bytes());
            if name_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
            }
            normalized.push(MoltObject::from_ptr(name_ptr).bits());
        }

        let result_ptr = alloc_tuple(_py, &normalized);
        // Decref the individual name strings (tuple took ownership via ref)
        for bits in &normalized {
            dec_ref_bits(_py, *bits);
        }
        if result_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate tuple");
        }
        MoltObject::from_ptr(result_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deque_size_order_mutation_invalidates_snapshot_index() {
        let mut state = DequeState::new(None);
        state.data.push_back(11);
        state.data.push_back(22);

        let snapshot_version = state.mutation_version;
        let found_idx = 0;

        state.data.clear();
        state.mark_size_or_order_mutation();

        assert_ne!(state.mutation_version, snapshot_version);
        assert!(state.data.get(found_idx).is_none());
    }

    #[test]
    fn deque_element_replacement_does_not_invalidate_equality_scan() {
        let mut state = DequeState::new(None);
        state.data.push_back(11);

        let snapshot_version = state.mutation_version;
        state.data[0] = 22;

        assert_eq!(state.mutation_version, snapshot_version);
    }
}
