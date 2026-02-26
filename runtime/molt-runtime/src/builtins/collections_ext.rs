// === FILE: runtime/molt-runtime/src/builtins/collections_ext.rs ===
//
// Intrinsic implementations for collections.OrderedDict and collections.ChainMap.
//
// Handle model: global Mutex<HashMap<i64, State>> keyed by an atomically-issued
// handle ID, returned to Python as a NaN-boxed integer. Uses a global registry
// (not thread-local) so handles are visible across all threads — critical for
// collections used cross-thread (e.g. deque in queue.Queue, concurrent.futures).
// The GIL serializes all Python-level access, so the Mutex is always uncontended.
//
// dict_order() is a flattened Vec<u64> of [key0, val0, key1, val1, ...] that
// is the canonical ordered representation of a Molt dict object.

use crate::*;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ─── Handle counters ──────────────────────────────────────────────────────────

static NEXT_ORDEREDDICT_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_CHAINMAP_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_ordereddict_handle() -> i64 {
    NEXT_ORDEREDDICT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_chainmap_handle() -> i64 {
    NEXT_CHAINMAP_HANDLE.fetch_add(1, Ordering::Relaxed)
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

// Global registry — visible across all threads.  GIL serializes access.
static ORDEREDDICT_REGISTRY: LazyLock<Mutex<HashMap<i64, OrderedDictState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── helpers ─────────────────────────────────────────────────────────────────

fn od_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
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
    crate::with_gil_entry!(_py, {
        let id = next_ordereddict_handle();
        ORDEREDDICT_REGISTRY
            .lock()
            .unwrap()
            .insert(id, OrderedDictState::new());
        MoltObject::from_int(id).bits()
    })
}

/// Create an OrderedDict from a list of (k, v) 2-tuples. Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_from_pairs(pairs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(pairs_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected a list of pairs");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "expected a list of pairs");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
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
            let pair = unsafe { seq_vec_ref(elem_ptr) };
            if pair.len() < 2 {
                return raise_exception::<_>(_py, "ValueError", "each pair must have 2 elements");
            }
            state.insert(pair[0], pair[1]);
        }
        let id = next_ordereddict_handle();
        ORDEREDDICT_REGISTRY.lock().unwrap().insert(id, state);
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = ORDEREDDICT_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let val = ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let found = {
            ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let len = ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let keys: Vec<u64> = {
            ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let vals: Vec<u64> = {
            ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let pairs: Vec<(u64, u64)> = {
            ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let last = is_truthy(_py, obj_from_bits(last_bits));
        let found = {
            let mut map = ORDEREDDICT_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let last = is_truthy(_py, obj_from_bits(last_bits));
        let item = ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = ORDEREDDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let other_obj = obj_from_bits(other_bits);
        // Accept either a regular Molt dict or another OrderedDict handle (int).
        if let Some(other_id) = to_i64(other_obj) {
            // Treat as another OrderedDict handle.
            let pairs: Vec<(u64, u64)> = {
                ORDEREDDICT_REGISTRY
                    .lock()
                    .unwrap()
                    .get(&other_id)
                    .map(|s| s.order.clone())
                    .unwrap_or_default()
            };
            {
                let mut map = ORDEREDDICT_REGISTRY.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for (k, v) in pairs {
                        state.insert(k, v);
                    }
                }
            }
        } else if let Some(ptr) = other_obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_DICT {
                let pairs = unsafe { dict_order(ptr).clone() };
                // pairs is flattened [k0, v0, k1, v1, ...]
                let kv_pairs: Vec<(u64, u64)> =
                    pairs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                {
                    let mut map = ORDEREDDICT_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = ORDEREDDICT_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let cloned = ORDEREDDICT_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.clone_state());
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid OrderedDict handle");
        };
        let new_id = next_ordereddict_handle();
        ORDEREDDICT_REGISTRY
            .lock()
            .unwrap()
            .insert(new_id, new_state);
        MoltObject::from_int(new_id).bits()
    })
}

/// Release handle resources. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ordereddict_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = od_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        ORDEREDDICT_REGISTRY.lock().unwrap().remove(&id);
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

// Global registry — visible across all threads.  GIL serializes access.
static CHAINMAP_REGISTRY: LazyLock<Mutex<HashMap<i64, ChainMapState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cm_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
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
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(maps_bits);
        let mut map_list: Vec<u64> = Vec::new();
        if let Some(ptr) = obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = unsafe { seq_vec_ref(ptr) };
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
        CHAINMAP_REGISTRY
            .lock()
            .unwrap()
            .insert(id, ChainMapState { maps: map_list });
        MoltObject::from_int(id).bits()
    })
}

/// Search all maps in order for key. Returns value or raises KeyError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_getitem(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = CHAINMAP_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let first_map_bits = CHAINMAP_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let first_map_bits = CHAINMAP_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = CHAINMAP_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = CHAINMAP_REGISTRY
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
            let order = unsafe { dict_order(dict_ptr) };
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = CHAINMAP_REGISTRY
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
            let order = unsafe { dict_order(dict_ptr) };
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let existing_maps: Vec<u64> = CHAINMAP_REGISTRY
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
        CHAINMAP_REGISTRY
            .lock()
            .unwrap()
            .insert(new_id, ChainMapState { maps: new_maps });
        MoltObject::from_int(new_id).bits()
    })
}

/// Return a new ChainMap containing all maps except the first. Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_parents(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let mut maps: Vec<u64> = CHAINMAP_REGISTRY
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
        CHAINMAP_REGISTRY
            .lock()
            .unwrap()
            .insert(new_id, ChainMapState { maps });
        MoltObject::from_int(new_id).bits()
    })
}

/// Return list of underlying dict objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_chainmap_maps(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maps: Vec<u64> = CHAINMAP_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = cm_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        CHAINMAP_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}
// ─── Handle counter ─────────────────────────────────────────────────────────

static NEXT_DEQUE_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_deque_handle() -> i64 {
    NEXT_DEQUE_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─── Deque state ────────────────────────────────────────────────────────────

struct DequeState {
    data: VecDeque<u64>,
    maxlen: Option<usize>,
}

// Global registry — NOT thread_local. Deques must be visible across threads
// (e.g. queue.Queue, concurrent.futures use deque cross-thread). The GIL
// serializes all Python-level access, so this Mutex is always uncontended.
static DEQUE_REGISTRY: LazyLock<Mutex<HashMap<i64, DequeState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── helpers ────────────────────────────────────────────────────────────────

fn deque_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "deque handle must be an int");
        return None;
    };
    Some(id)
}

/// Parse maxlen_bits into Option<usize>.
/// Returns Ok(None) for Python None (unbounded), Ok(Some(n)) for non-negative int,
/// or Err(()) after raising ValueError for negative.
fn parse_maxlen(_py: &PyToken<'_>, maxlen_bits: u64) -> Result<Option<usize>, ()> {
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
fn extract_iterable_elements(_py: &PyToken<'_>, iterable_bits: u64) -> Option<&'static Vec<u64>> {
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
    Some(unsafe { seq_vec_ref(ptr) })
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
    crate::with_gil_entry!(_py, {
        let maxlen = match parse_maxlen(_py, maxlen_bits) {
            Ok(m) => m,
            Err(()) => return MoltObject::none().bits(),
        };
        let id = next_deque_handle();
        DEQUE_REGISTRY.lock().unwrap().insert(
            id,
            DequeState {
                data: VecDeque::new(),
                maxlen,
            },
        );
        MoltObject::from_int(id).bits()
    })
}

/// Create a deque from an iterable (list/tuple) with optional maxlen.
/// If maxlen is set and the iterable is longer, keeps only the last maxlen elements.
/// Returns handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_from_iterable(iterable_bits: u64, maxlen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxlen = match parse_maxlen(_py, maxlen_bits) {
            Ok(m) => m,
            Err(()) => return MoltObject::none().bits(),
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        let data: VecDeque<u64> = if let Some(ml) = maxlen {
            // If bounded and iterable longer than maxlen, keep only the last maxlen elements.
            if elems.len() > ml {
                elems[elems.len() - ml..].iter().copied().collect()
            } else {
                elems.iter().copied().collect()
            }
        } else {
            elems.iter().copied().collect()
        };
        let id = next_deque_handle();
        DEQUE_REGISTRY
            .lock()
            .unwrap()
            .insert(id, DequeState { data, maxlen });
        MoltObject::from_int(id).bits()
    })
}

/// Append item to the right end. If bounded and full, pop from the left.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_append(handle_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(ml) = state.maxlen {
                    if ml == 0 {
                        // maxlen is 0, element is silently dropped.
                        return MoltObject::none().bits();
                    }
                    if state.data.len() == ml {
                        state.data.pop_front();
                    }
                }
                state.data.push_back(item_bits);
            }
        }
        MoltObject::none().bits()
    })
}

/// Append item to the left end. If bounded and full, pop from the right.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_appendleft(handle_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(ml) = state.maxlen {
                    if ml == 0 {
                        return MoltObject::none().bits();
                    }
                    if state.data.len() == ml {
                        state.data.pop_back();
                    }
                }
                state.data.push_front(item_bits);
            }
        }
        MoltObject::none().bits()
    })
}

/// Remove and return the rightmost element.
/// Raises IndexError if the deque is empty.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_pop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let result = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.data.pop_back());
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let result = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.data.pop_front());
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        // Clone the elements to avoid holding the seq_vec_ref borrow across the mutation.
        let elems_owned: Vec<u64> = elems.clone();
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                for &item in &elems_owned {
                    if let Some(ml) = state.maxlen {
                        if ml == 0 {
                            continue;
                        }
                        if state.data.len() == ml {
                            state.data.pop_front();
                        }
                    }
                    state.data.push_back(item);
                }
            }
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(elems) = extract_iterable_elements(_py, iterable_bits) else {
            return MoltObject::none().bits();
        };
        let elems_owned: Vec<u64> = elems.clone();
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                // Each element is prepended in order, which reverses the iterable.
                for &item in &elems_owned {
                    if let Some(ml) = state.maxlen {
                        if ml == 0 {
                            continue;
                        }
                        if state.data.len() == ml {
                            state.data.pop_back();
                        }
                    }
                    state.data.push_front(item);
                }
            }
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let n_obj = obj_from_bits(n_bits);
        let Some(n) = to_i64(n_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
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
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// Return the length of the deque as a NaN-boxed int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let len = DEQUE_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let idx_obj = obj_from_bits(index_bits);
        let Some(index) = to_i64(idx_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let result = {
            let map = DEQUE_REGISTRY.lock().unwrap();
            map.get(&id).and_then(|state| {
                let resolved = resolve_index(index, state.data.len())?;
                state.data.get(resolved).copied()
            })
        };
        match result {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "IndexError", "deque index out of range"),
        }
    })
}

/// Set element at index. Supports negative indexing.
/// Raises IndexError if out of bounds.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_setitem(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let idx_obj = obj_from_bits(index_bits);
        let Some(index) = to_i64(idx_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let ok = {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(resolved) = resolve_index(index, state.data.len()) {
                    state.data[resolved] = value_bits;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !ok {
            return raise_exception::<_>(_py, "IndexError", "deque index out of range");
        }
        MoltObject::none().bits()
    })
}

/// Delete element at index. Supports negative indexing.
/// Uses VecDeque::remove() which is O(min(i, n-i)).
/// Raises IndexError if out of bounds.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_delitem(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let idx_obj = obj_from_bits(index_bits);
        let Some(index) = to_i64(idx_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let ok = {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                if let Some(resolved) = resolve_index(index, state.data.len()) {
                    state.data.remove(resolved);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !ok {
            return raise_exception::<_>(_py, "IndexError", "deque index out of range");
        }
        MoltObject::none().bits()
    })
}

/// Return True if item is found in the deque via obj_eq comparison.
/// Uses iterator, no allocation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_contains(handle_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        // Snapshot the elements to avoid holding the lock during obj_eq calls,
        // which may re-enter the runtime.
        let elements: Vec<u64> = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.data.iter().copied().collect())
            .unwrap_or_default();
        let target = obj_from_bits(item_bits);
        for &elem_bits in &elements {
            if obj_eq(_py, obj_from_bits(elem_bits), target) {
                return MoltObject::from_bool(true).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

/// Count elements equal to item via obj_eq. Returns count as NaN-boxed int.
/// Uses iterator, no allocation beyond the snapshot.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_count(handle_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let elements: Vec<u64> = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.data.iter().copied().collect())
            .unwrap_or_default();
        let target = obj_from_bits(item_bits);
        let mut count: i64 = 0;
        for &elem_bits in &elements {
            if obj_eq(_py, obj_from_bits(elem_bits), target) {
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
    crate::with_gil_entry!(_py, {
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
        // Snapshot elements to avoid holding the lock during obj_eq.
        let elements: Vec<u64> = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.data.iter().copied().collect())
            .unwrap_or_default();
        let len = elements.len() as i64;
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
        for (i, &elem_bits) in elements
            .iter()
            .enumerate()
            .take(stop_usize)
            .skip(start_usize)
        {
            if obj_eq(_py, obj_from_bits(elem_bits), target) {
                return MoltObject::from_int(i as i64).bits();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let idx_obj = obj_from_bits(index_bits);
        let Some(index) = to_i64(idx_obj) else {
            return raise_exception::<_>(_py, "TypeError", "integer argument expected");
        };
        let ok = {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
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
                        state.data.insert(resolved as usize, item_bits);
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
                    state.data.insert(resolved as usize, item_bits);
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        // Snapshot to find the index without holding the lock during obj_eq.
        let elements: Vec<u64> = DEQUE_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.data.iter().copied().collect())
            .unwrap_or_default();
        let target = obj_from_bits(item_bits);
        let mut found_idx: Option<usize> = None;
        for (i, &elem_bits) in elements.iter().enumerate() {
            if obj_eq(_py, obj_from_bits(elem_bits), target) {
                found_idx = Some(i);
                break;
            }
        }
        let Some(idx) = found_idx else {
            return raise_exception::<_>(_py, "ValueError", "deque.remove(x): x not in deque");
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.data.remove(idx);
            }
        }
        MoltObject::none().bits()
    })
}

/// Reverse the deque in place.
/// Uses make_contiguous() + reverse() for performance.
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_reverse(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = DEQUE_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.data.clear();
            }
        }
        MoltObject::none().bits()
    })
}

/// Create a shallow copy. Uses VecDeque::clone() — no element-by-element copy.
/// Returns a new handle with the same maxlen.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_copy(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let cloned = DEQUE_REGISTRY.lock().unwrap().get(&id).map(|s| DequeState {
            data: s.data.clone(),
            maxlen: s.maxlen,
        });
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid deque handle");
        };
        let new_id = next_deque_handle();
        DEQUE_REGISTRY.lock().unwrap().insert(new_id, new_state);
        MoltObject::from_int(new_id).bits()
    })
}

/// Return maxlen as NaN-boxed int, or None if unbounded.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deque_maxlen(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let maxlen = DEQUE_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = deque_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        DEQUE_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}

// ─── Counter intrinsics ─────────────────────────────────────────────────────
//
// Handle model: global Mutex<HashMap<i64, CounterState>> keyed by an atomically-
// issued handle ID, returned to Python as a NaN-boxed integer.  Matches the
// pattern established by the other collection types in this file.
//
// Internal state: Vec<(u64, i64)> for insertion-order.
// Key lookup uses content-based equality (obj_eq) to correctly handle
// heap-allocated values like strings where identical content may have
// different NaN-boxed bit patterns.

// ─── Counter state ─────────────────────────────────────────────────────────

struct CounterState {
    /// Insertion-ordered (element_bits, count_bits) pairs.
    /// `count_bits` is NaN-boxed: usually an int, but can be any value (e.g. float)
    /// when assigned via `__setitem__`.
    entries: Vec<(u64, u64)>,
}

/// Extract an i64 count from NaN-boxed bits, defaulting to 0 for non-integers.
#[inline]
fn count_to_i64(bits: u64) -> i64 {
    to_i64(obj_from_bits(bits)).unwrap_or(0)
}

/// Convert an i64 count to NaN-boxed bits.
#[inline]
fn i64_to_count(n: i64) -> u64 {
    MoltObject::from_int(n).bits()
}

impl CounterState {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Find the index of `key` using content-based equality.
    /// Fast path: bit-exact match first, then obj_eq for heap values.
    #[inline]
    fn find_key(&self, _py: &PyToken<'_>, key: u64) -> Option<usize> {
        let target = obj_from_bits(key);
        for (i, &(k, _)) in self.entries.iter().enumerate() {
            if k == key || obj_eq(_py, obj_from_bits(k), target) {
                return Some(i);
            }
        }
        None
    }

    /// Return raw NaN-boxed count bits for `key` (0 if missing).
    #[inline]
    fn get_count_bits(&self, _py: &PyToken<'_>, key: u64) -> u64 {
        self.find_key(_py, key)
            .map(|i| self.entries[i].1)
            .unwrap_or_else(|| i64_to_count(0))
    }

    /// Add an integer delta to the count for `key`.
    /// If the existing count is non-integer, it is treated as 0.
    #[inline]
    fn add_count(&mut self, _py: &PyToken<'_>, key: u64, delta: i64) {
        if let Some(idx) = self.find_key(_py, key) {
            let cur = count_to_i64(self.entries[idx].1);
            self.entries[idx].1 = i64_to_count(cur + delta);
        } else {
            inc_ref_bits(_py, key);
            self.entries.push((key, i64_to_count(delta)));
        }
    }

    /// Store raw NaN-boxed bits as the count for `key` (used by `__setitem__`).
    #[inline]
    fn set_count_raw(&mut self, _py: &PyToken<'_>, key: u64, count_bits: u64) {
        if let Some(idx) = self.find_key(_py, key) {
            self.entries[idx].1 = count_bits;
        } else {
            inc_ref_bits(_py, key);
            self.entries.push((key, count_bits));
        }
    }

    /// Store an i64 count for `key` (used by `from_mapping`).
    #[inline]
    fn set_count_i64(&mut self, _py: &PyToken<'_>, key: u64, count: i64) {
        self.set_count_raw(_py, key, i64_to_count(count));
    }

    fn remove(&mut self, _py: &PyToken<'_>, key: u64) -> Option<u64> {
        let idx = self.find_key(_py, key)?;
        let (old_key, count_bits) = self.entries.swap_remove(idx);
        dec_ref_bits(_py, old_key);
        Some(count_bits)
    }

    #[inline]
    fn contains(&self, _py: &PyToken<'_>, key: u64) -> bool {
        self.find_key(_py, key).is_some()
    }

    #[inline]
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn clear(&mut self, _py: &PyToken<'_>) {
        for &(key, _) in &self.entries {
            dec_ref_bits(_py, key);
        }
        self.entries.clear();
    }

    fn clone_state(&self, _py: &PyToken<'_>) -> Self {
        for &(key, _) in &self.entries {
            inc_ref_bits(_py, key);
        }
        Self {
            entries: self.entries.clone(),
        }
    }
}

// ─── defaultdict state ─────────────────────────────────────────────────────

struct DefaultDictState {
    factory_bits: u64,
}

// ─── Handle counters ────────────────────────────────────────────────────────

static NEXT_COUNTER_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_DEFAULTDICT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_counter_handle() -> i64 {
    NEXT_COUNTER_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_defaultdict_handle() -> i64 {
    NEXT_DEFAULTDICT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─── Global handle registries ────────────────────────────────────────────────
// Visible across all threads.  GIL serializes access.

static COUNTER_REGISTRY: LazyLock<Mutex<HashMap<i64, CounterState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DEFAULTDICT_REGISTRY: LazyLock<Mutex<HashMap<i64, DefaultDictState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Handle helpers ─────────────────────────────────────────────────────────

fn counter_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "Counter handle must be an int");
        return None;
    };
    Some(id)
}

fn dd_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "defaultdict handle must be an int");
        return None;
    };
    Some(id)
}

// ─── Counter intrinsics: construction ───────────────────────────────────────

/// Create an empty Counter.  Returns an integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let id = next_counter_handle();
        COUNTER_REGISTRY
            .lock()
            .unwrap()
            .insert(id, CounterState::new());
        MoltObject::from_int(id).bits()
    })
}

/// Create a Counter by counting elements from a list/tuple iterable.
/// Each element becomes a key with count incremented by 1.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_from_iterable(iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(iterable_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected a list or tuple");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "expected a list or tuple");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        let mut state = CounterState::new();
        for &elem_bits in elems.iter() {
            state.add_count(_py, elem_bits, 1);
        }
        let id = next_counter_handle();
        COUNTER_REGISTRY.lock().unwrap().insert(id, state);
        MoltObject::from_int(id).bits()
    })
}

/// Create a Counter from a list of (key, count) pairs.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_from_mapping(mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(mapping_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected a list of (key, count) pairs");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "expected a list of (key, count) pairs");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        let mut state = CounterState::new();
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let Some(elem_ptr) = elem_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
            };
            let elem_type = unsafe { object_type_id(elem_ptr) };
            if elem_type != TYPE_ID_TUPLE && elem_type != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
            }
            let pair = unsafe { seq_vec_ref(elem_ptr) };
            if pair.len() < 2 {
                return raise_exception::<_>(_py, "ValueError", "each pair must have 2 elements");
            }
            let count_obj = obj_from_bits(pair[1]);
            let Some(count) = to_i64(count_obj) else {
                return raise_exception::<_>(_py, "TypeError", "count must be an integer");
            };
            state.set_count_i64(_py, pair[0], count);
        }
        let id = next_counter_handle();
        COUNTER_REGISTRY.lock().unwrap().insert(id, state);
        MoltObject::from_int(id).bits()
    })
}

// ─── Counter intrinsics: item access ────────────────────────────────────────

/// Return count for key.  Missing keys return 0 (not KeyError).
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_getitem(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.get_count_bits(_py, key_bits))
            .unwrap_or_else(|| i64_to_count(0))
    })
}

/// Set count for key.  Accepts any NaN-boxed value (int, float, etc.).
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_setitem(handle_bits: u64, key_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = COUNTER_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.set_count_raw(_py, key_bits, count_bits);
            }
        }
        MoltObject::none().bits()
    })
}

/// Delete key.  Raises KeyError if not found.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_delitem(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.remove(_py, key_bits));
        if removed.is_none() {
            return raise_key_error_with_key::<u64>(_py, key_bits);
        }
        MoltObject::none().bits()
    })
}

// ─── Counter intrinsics: query ──────────────────────────────────────────────

/// Return a flat list of elements, each repeated by its count.
/// Elements with count <= 0 are skipped.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_elements(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let items: Vec<u64> = {
            let map = COUNTER_REGISTRY.lock().unwrap();
            match map.get(&id) {
                None => Vec::new(),
                Some(state) => {
                    let mut out = Vec::new();
                    for &(elem_bits, count_bits) in &state.entries {
                        let count = count_to_i64(count_bits);
                        if count > 0 {
                            for _ in 0..count {
                                out.push(elem_bits);
                            }
                        }
                    }
                    out
                }
            }
        };
        let ptr = alloc_list(_py, &items);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Return (element, count) pairs sorted by count descending.
/// If n_bits is None, return ALL pairs.  If n_bits is an int, return top n.
/// Uses stable sort; for ties, insertion order is preserved.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_most_common(handle_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let n_obj = obj_from_bits(n_bits);
        let n_limit: Option<usize> = if n_obj.is_none() {
            None
        } else {
            let Some(n) = to_i64(n_obj) else {
                return raise_exception::<_>(_py, "TypeError", "n must be an integer or None");
            };
            if n < 0 { Some(0) } else { Some(n as usize) }
        };

        let mut pairs: Vec<(u64, u64)> = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.entries.clone())
            .unwrap_or_default();

        let cmp_count = |a: &(u64, u64), b: &(u64, u64)| count_to_i64(b.1).cmp(&count_to_i64(a.1));

        // Always use stable sort to preserve insertion order for equal counts.
        match n_limit {
            Some(0) => {
                pairs.clear();
            }
            _ => {
                pairs.sort_by(cmp_count);
                if let Some(n) = n_limit {
                    pairs.truncate(n);
                }
            }
        }

        let mut tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
        for (elem_bits, count_bits) in &pairs {
            let tptr = alloc_tuple(_py, &[*elem_bits, *count_bits]);
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

/// Sum all counts.  Returns as NaN-boxed int.  No allocation for iteration.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_total(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let total: i64 = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.entries.iter().map(|(_, c)| count_to_i64(*c)).sum())
            .unwrap_or(0);
        MoltObject::from_int(total).bits()
    })
}

// ─── Counter intrinsics: mutation ───────────────────────────────────────────

/// Update counter from source.
/// If source is a flat list: count each element (+1 per occurrence).
/// If source is a list of 2-tuples: add count for each key.
/// Detection: check if first element is a tuple.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_update(handle_bits: u64, source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let src_obj = obj_from_bits(source_bits);
        let Some(src_ptr) = src_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "update source must be a list or tuple");
        };
        let src_type = unsafe { object_type_id(src_ptr) };
        if src_type != TYPE_ID_LIST && src_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "update source must be a list or tuple");
        }
        let elems = unsafe { seq_vec_ref(src_ptr) }.clone();
        if elems.is_empty() {
            return MoltObject::none().bits();
        }

        let first_obj = obj_from_bits(elems[0]);
        let is_mapping = first_obj.as_ptr().is_some_and(|fptr| {
            let ft = unsafe { object_type_id(fptr) };
            ft == TYPE_ID_TUPLE || ft == TYPE_ID_LIST
        });

        if is_mapping {
            let mut deltas: Vec<(u64, i64)> = Vec::with_capacity(elems.len());
            for &elem_bits in &elems {
                let elem_obj = obj_from_bits(elem_bits);
                let Some(elem_ptr) = elem_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
                };
                let elem_type = unsafe { object_type_id(elem_ptr) };
                if elem_type != TYPE_ID_TUPLE && elem_type != TYPE_ID_LIST {
                    return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
                }
                let pair = unsafe { seq_vec_ref(elem_ptr) };
                if pair.len() < 2 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "each pair must have 2 elements",
                    );
                }
                let count_obj = obj_from_bits(pair[1]);
                let Some(count) = to_i64(count_obj) else {
                    return raise_exception::<_>(_py, "TypeError", "count must be an integer");
                };
                deltas.push((pair[0], count));
            }
            {
                let mut map = COUNTER_REGISTRY.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for (key, delta) in deltas {
                        state.add_count(_py, key, delta);
                    }
                }
            }
        } else {
            {
                let mut map = COUNTER_REGISTRY.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for &elem_bits in &elems {
                        state.add_count(_py, elem_bits, 1);
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// Subtract counts from source.  Same detection logic as update.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_subtract(handle_bits: u64, source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let src_obj = obj_from_bits(source_bits);
        let Some(src_ptr) = src_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "subtract source must be a list or tuple",
            );
        };
        let src_type = unsafe { object_type_id(src_ptr) };
        if src_type != TYPE_ID_LIST && src_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "subtract source must be a list or tuple",
            );
        }
        let elems = unsafe { seq_vec_ref(src_ptr) }.clone();
        if elems.is_empty() {
            return MoltObject::none().bits();
        }

        let first_obj = obj_from_bits(elems[0]);
        let is_mapping = first_obj.as_ptr().is_some_and(|fptr| {
            let ft = unsafe { object_type_id(fptr) };
            ft == TYPE_ID_TUPLE || ft == TYPE_ID_LIST
        });

        if is_mapping {
            let mut deltas: Vec<(u64, i64)> = Vec::with_capacity(elems.len());
            for &elem_bits in &elems {
                let elem_obj = obj_from_bits(elem_bits);
                let Some(elem_ptr) = elem_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
                };
                let elem_type = unsafe { object_type_id(elem_ptr) };
                if elem_type != TYPE_ID_TUPLE && elem_type != TYPE_ID_LIST {
                    return raise_exception::<_>(_py, "TypeError", "each pair must be a tuple");
                }
                let pair = unsafe { seq_vec_ref(elem_ptr) };
                if pair.len() < 2 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "each pair must have 2 elements",
                    );
                }
                let count_obj = obj_from_bits(pair[1]);
                let Some(count) = to_i64(count_obj) else {
                    return raise_exception::<_>(_py, "TypeError", "count must be an integer");
                };
                deltas.push((pair[0], count));
            }
            {
                let mut map = COUNTER_REGISTRY.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for (key, delta) in deltas {
                        state.add_count(_py, key, -delta);
                    }
                }
            }
        } else {
            {
                let mut map = COUNTER_REGISTRY.lock().unwrap();
                if let Some(state) = map.get_mut(&id) {
                    for &elem_bits in &elems {
                        state.add_count(_py, elem_bits, -1);
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

// ─── Counter intrinsics: iteration / inspection ─────────────────────────────

/// Return list of (element, count) 2-tuples in insertion order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_items(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let pairs: Vec<(u64, u64)> = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.entries.clone())
            .unwrap_or_default();
        let mut tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
        for (elem_bits, count_bits) in &pairs {
            let tptr = alloc_tuple(_py, &[*elem_bits, *count_bits]);
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

/// Return number of unique elements.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let len = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.len())
            .unwrap_or(0);
        MoltObject::from_int(len as i64).bits()
    })
}

/// Return True if key exists in counter, False otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_contains(handle_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let found = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.contains(_py, key_bits))
            .unwrap_or(false);
        MoltObject::from_bool(found).bits()
    })
}

// ─── Counter intrinsics: arithmetic (binary, produce new Counter) ───────────

/// Helper: collect all unique keys from two counters without allocating a
/// separate HashSet — reuse the new counter's index for dedup.
fn counter_binary_op(
    _py: &PyToken<'_>,
    a_bits: u64,
    b_bits: u64,
    combine: fn(i64, i64) -> i64,
) -> u64 {
    let Some(a_id) = counter_handle_from_bits(_py, a_bits) else {
        return MoltObject::none().bits();
    };
    let Some(b_id) = counter_handle_from_bits(_py, b_bits) else {
        return MoltObject::none().bits();
    };

    let a_entries: Vec<(u64, u64)> = COUNTER_REGISTRY
        .lock()
        .unwrap()
        .get(&a_id)
        .map(|s| s.entries.clone())
        .unwrap_or_default();
    let b_entries: Vec<(u64, u64)> = COUNTER_REGISTRY
        .lock()
        .unwrap()
        .get(&b_id)
        .map(|s| s.entries.clone())
        .unwrap_or_default();

    let mut result = CounterState::new();
    let mut b_exact_index: HashMap<u64, usize> = HashMap::with_capacity(b_entries.len());
    for (idx, (key, _)) in b_entries.iter().copied().enumerate() {
        b_exact_index.entry(key).or_insert(idx);
    }
    let mut b_matched = vec![false; b_entries.len()];

    // Process keys from a: resolve b-count via exact-key fast path with content-equality fallback.
    for &(key, a_count_bits) in &a_entries {
        let a_count = count_to_i64(a_count_bits);
        let mut matched_idx = b_exact_index.get(&key).copied();
        if matched_idx.is_none() {
            let target = obj_from_bits(key);
            matched_idx = b_entries
                .iter()
                .enumerate()
                .find(|(_, (k, _))| obj_eq(_py, obj_from_bits(*k), target))
                .map(|(idx, _)| idx);
        }
        let b_count = if let Some(idx) = matched_idx {
            b_matched[idx] = true;
            count_to_i64(b_entries[idx].1)
        } else {
            0
        };
        let combined = combine(a_count, b_count);
        if combined > 0 {
            result.set_count_i64(_py, key, combined);
        }
    }

    // Process keys only present in b (keys matched in the a-pass are skipped even if
    // their combined count became non-positive and was filtered out).
    for (idx, &(key, b_count_bits)) in b_entries.iter().enumerate() {
        if b_matched[idx] {
            continue;
        }
        let combined = combine(0, count_to_i64(b_count_bits));
        if combined > 0 {
            result.set_count_i64(_py, key, combined);
        }
    }

    let id = next_counter_handle();
    COUNTER_REGISTRY.lock().unwrap().insert(id, result);
    MoltObject::from_int(id).bits()
}

/// c + d: For each key, sum counts.  Only keep positive counts.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_add(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        counter_binary_op(_py, a_bits, b_bits, |a, b| a + b)
    })
}

/// c - d: For each key, subtract counts.  Only keep positive counts.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_sub(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        counter_binary_op(_py, a_bits, b_bits, |a, b| a - b)
    })
}

/// c | d: Union — max(c[x], d[x]) for each key.  Only keep positive counts.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_or(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        counter_binary_op(_py, a_bits, b_bits, |a, b| a.max(b))
    })
}

/// c & d: Intersection — min(c[x], d[x]) for each key.  Only keep positive.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_and(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        counter_binary_op(_py, a_bits, b_bits, |a, b| a.min(b))
    })
}

// ─── Counter intrinsics: copy / clear / pop / drop ──────────────────────────

/// Deep copy.  Returns new handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_copy(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let cloned = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.clone_state(_py));
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid Counter handle");
        };
        let new_id = next_counter_handle();
        COUNTER_REGISTRY.lock().unwrap().insert(new_id, new_state);
        MoltObject::from_int(new_id).bits()
    })
}

/// Clear all entries.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = COUNTER_REGISTRY.lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                state.clear(_py);
            }
        }
        MoltObject::none().bits()
    })
}

/// Remove key and return its count.  If key not found and default_bits is not
/// None, return default.  Otherwise raise KeyError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_pop(handle_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = COUNTER_REGISTRY
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|s| s.remove(_py, key_bits));
        match removed {
            Some(count_bits) => count_bits,
            None => {
                let default_obj = obj_from_bits(default_bits);
                if default_obj.is_none() {
                    raise_key_error_with_key::<u64>(_py, key_bits)
                } else {
                    default_bits
                }
            }
        }
    })
}

/// Release handle resources.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_counter_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = counter_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let removed = COUNTER_REGISTRY.lock().unwrap().remove(&id);
        if let Some(state) = removed {
            for &(key, _) in &state.entries {
                dec_ref_bits(_py, key);
            }
        }
        MoltObject::none().bits()
    })
}

// ─── defaultdict intrinsics ─────────────────────────────────────────────────

/// Create a new defaultdict handle storing the factory callable (or None).
/// Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_new(factory_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = next_defaultdict_handle();
        DEFAULTDICT_REGISTRY
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
    crate::with_gil_entry!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let factory = DEFAULTDICT_REGISTRY
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
        // Call the factory with 0 arguments (supports types, functions, bound methods).
        let val = unsafe { crate::call::dispatch::call_callable0(_py, factory_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        val
    })
}

/// Return the factory_bits.  If None, return None bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_factory(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        DEFAULTDICT_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.factory_bits)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

/// Create new handle with the same factory_bits.  Returns new handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_copy(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let factory = DEFAULTDICT_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.factory_bits);
        let Some(factory_bits) = factory else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid defaultdict handle");
        };
        let new_id = next_defaultdict_handle();
        DEFAULTDICT_REGISTRY
            .lock()
            .unwrap()
            .insert(new_id, DefaultDictState { factory_bits });
        MoltObject::from_int(new_id).bits()
    })
}

/// Release defaultdict handle.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_defaultdict_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = dd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        DEFAULTDICT_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}
