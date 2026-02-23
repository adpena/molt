// === FILE: runtime/molt-runtime/src/builtins/collections_ext.rs ===
//
// Intrinsic implementations for collections.OrderedDict and collections.ChainMap.
//
// Handle model: thread-local HashMap<i64, State> keyed by an atomically-issued
// handle ID, returned to Python as a NaN-boxed integer. Matches the pattern
// established by builtins/csv.rs.
//
// dict_order() is a flattened Vec<u64> of [key0, val0, key1, val1, ...] that
// is the canonical ordered representation of a Molt dict object.

use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

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

thread_local! {
    static ORDEREDDICT_HANDLES: RefCell<HashMap<i64, OrderedDictState>> =
        RefCell::new(HashMap::new());
}

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
        ORDEREDDICT_HANDLES.with(|h| h.borrow_mut().insert(id, OrderedDictState::new()));
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
        ORDEREDDICT_HANDLES.with(|h| h.borrow_mut().insert(id, state));
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
        ORDEREDDICT_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            if let Some(state) = map.get_mut(&id) {
                state.insert(key_bits, value_bits);
            }
        });
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
        let val = ORDEREDDICT_HANDLES.with(|h| h.borrow().get(&id).and_then(|s| s.get(key_bits)));
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
        let removed = ORDEREDDICT_HANDLES
            .with(|h| h.borrow_mut().get_mut(&id).and_then(|s| s.remove(key_bits)));
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
        let found = ORDEREDDICT_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.contains(key_bits))
                .unwrap_or(false)
        });
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
        let len = ORDEREDDICT_HANDLES.with(|h| h.borrow().get(&id).map(|s| s.len()).unwrap_or(0));
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
        let keys: Vec<u64> = ORDEREDDICT_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.order.iter().map(|(k, _)| *k).collect())
                .unwrap_or_default()
        });
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
        let vals: Vec<u64> = ORDEREDDICT_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.order.iter().map(|(_, v)| *v).collect())
                .unwrap_or_default()
        });
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
        let pairs: Vec<(u64, u64)> = ORDEREDDICT_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.order.clone())
                .unwrap_or_default()
        });
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
        let found = ORDEREDDICT_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
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
        });
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
        let item =
            ORDEREDDICT_HANDLES.with(|h| h.borrow_mut().get_mut(&id).and_then(|s| s.popitem(last)));
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
        let removed = ORDEREDDICT_HANDLES
            .with(|h| h.borrow_mut().get_mut(&id).and_then(|s| s.remove(key_bits)));
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
            let pairs: Vec<(u64, u64)> = ORDEREDDICT_HANDLES.with(|h| {
                h.borrow()
                    .get(&other_id)
                    .map(|s| s.order.clone())
                    .unwrap_or_default()
            });
            ORDEREDDICT_HANDLES.with(|h| {
                if let Some(state) = h.borrow_mut().get_mut(&id) {
                    for (k, v) in pairs {
                        state.insert(k, v);
                    }
                }
            });
        } else if let Some(ptr) = other_obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_DICT {
                let pairs = unsafe { dict_order(ptr).clone() };
                // pairs is flattened [k0, v0, k1, v1, ...]
                let kv_pairs: Vec<(u64, u64)> =
                    pairs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                ORDEREDDICT_HANDLES.with(|h| {
                    if let Some(state) = h.borrow_mut().get_mut(&id) {
                        for (k, v) in kv_pairs {
                            state.insert(k, v);
                        }
                    }
                });
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
        ORDEREDDICT_HANDLES.with(|h| {
            if let Some(state) = h.borrow_mut().get_mut(&id) {
                state.clear();
            }
        });
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
        let cloned = ORDEREDDICT_HANDLES.with(|h| h.borrow().get(&id).map(|s| s.clone_state()));
        let Some(new_state) = cloned else {
            return raise_exception::<_>(_py, "RuntimeError", "invalid OrderedDict handle");
        };
        let new_id = next_ordereddict_handle();
        ORDEREDDICT_HANDLES.with(|h| h.borrow_mut().insert(new_id, new_state));
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
        ORDEREDDICT_HANDLES.with(|h| h.borrow_mut().remove(&id));
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

thread_local! {
    static CHAINMAP_HANDLES: RefCell<HashMap<i64, ChainMapState>> =
        RefCell::new(HashMap::new());
}

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
        CHAINMAP_HANDLES.with(|h| {
            h.borrow_mut().insert(id, ChainMapState { maps: map_list });
        });
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
        let maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        let first_map_bits =
            CHAINMAP_HANDLES.with(|h| h.borrow().get(&id).and_then(|s| s.maps.first().copied()));
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
        let first_map_bits =
            CHAINMAP_HANDLES.with(|h| h.borrow().get(&id).and_then(|s| s.maps.first().copied()));
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
        let maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        let maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        let maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        let existing_maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        CHAINMAP_HANDLES.with(|h| {
            h.borrow_mut()
                .insert(new_id, ChainMapState { maps: new_maps });
        });
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
        let mut maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        CHAINMAP_HANDLES.with(|h| {
            h.borrow_mut().insert(new_id, ChainMapState { maps });
        });
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
        let maps: Vec<u64> = CHAINMAP_HANDLES.with(|h| {
            h.borrow()
                .get(&id)
                .map(|s| s.maps.clone())
                .unwrap_or_default()
        });
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
        CHAINMAP_HANDLES.with(|h| h.borrow_mut().remove(&id));
        MoltObject::none().bits()
    })
}
