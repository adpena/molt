use crate::state::runtime_state::{
    WeakKeyDictEntry, WeakRefEntry, WeakSetEntry, WeakValueDictEntry,
};
use crate::{
    alloc_list, alloc_tuple, call_callable0, call_callable1, clear_exception, dec_ref_bits,
    dict_order, exception_pending, header_from_obj_ptr, inc_ref_bits, int_bits_from_i64, is_truthy,
    module_dict_bits, obj_from_bits, object_type_id, resolve_ptr, TYPE_ID_DICT, TYPE_ID_MODULE,
};
use crate::{
    molt_eq, molt_is_callable, raise_exception, runtime_state, type_name, MoltObject, PtrSlot,
    PyToken,
};
use std::ptr;
use std::sync::atomic::Ordering as AtomicOrdering;

pub(crate) fn weakref_clear_for_ptr(_py: &PyToken<'_>, target_ptr: *mut u8) {
    if target_ptr.is_null() {
        return;
    }
    let mut callbacks: Vec<(u64, u64)> = Vec::new();
    let target_slot = PtrSlot(target_ptr);
    {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(list) = registry.by_target.remove(&target_slot) else {
            return;
        };
        for weak_slot in list {
            if weak_slot.0.is_null() {
                continue;
            }
            let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                continue;
            };
            entry.target = PtrSlot(ptr::null_mut());
            let cb_bits = entry.callback_bits;
            if !obj_from_bits(cb_bits).is_none() {
                // CPython runs weakref callbacks at most once.
                entry.callback_bits = MoltObject::none().bits();
                let weak_bits = MoltObject::from_ptr(weak_slot.0).bits();
                inc_ref_bits(_py, cb_bits);
                inc_ref_bits(_py, weak_bits);
                callbacks.push((weak_bits, cb_bits));
            }
        }
    }
    for (weak_bits, cb_bits) in callbacks {
        let res_bits = unsafe { call_callable1(_py, cb_bits, weak_bits) };
        if exception_pending(_py) {
            clear_exception(_py);
        }
        if !obj_from_bits(res_bits).is_none() {
            dec_ref_bits(_py, res_bits);
        }
        dec_ref_bits(_py, cb_bits);
        dec_ref_bits(_py, weak_bits);
    }
}

pub(crate) fn weakref_collect_for_gc(_py: &PyToken<'_>) -> usize {
    let targets: Vec<*mut u8> = {
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let mut out: Vec<*mut u8> = Vec::new();
        for slot in registry.by_target.keys() {
            let ptr = slot.0;
            if ptr.is_null() {
                continue;
            }
            let addr = ptr.expose_provenance() as u64;
            if resolve_ptr(addr).is_none() {
                out.push(ptr);
                continue;
            }
            // Keep module-bound objects alive during explicit collections; all other
            // weakref targets are eligible for invalidation in collect().
            if !target_bound_in_module_globals(_py, ptr) {
                out.push(ptr);
            }
        }
        out
    };
    let collected = targets.len();
    for ptr in targets {
        weakref_clear_for_ptr(_py, ptr);
    }
    collected
}

pub(crate) fn weakref_run_atexit_finalizers(_py: &PyToken<'_>) {
    let mut finalizers: Vec<u64> = {
        let mut guard = runtime_state(_py).weakref_finalizers.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    while let Some(finalizer_bits) = finalizers.pop() {
        let res_bits = unsafe { call_callable0(_py, finalizer_bits) };
        if exception_pending(_py) {
            clear_exception(_py);
        }
        if !obj_from_bits(res_bits).is_none() {
            dec_ref_bits(_py, res_bits);
        }
        dec_ref_bits(_py, finalizer_bits);
    }
}

pub(crate) fn weakref_clear_container_state(_py: &PyToken<'_>) {
    {
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        for (_slot, entries) in guard.drain() {
            for entry in entries {
                weakkey_entry_drop(_py, entry);
            }
        }
    }
    {
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        for (_slot, entries) in guard.drain() {
            for entry in entries {
                weakvalue_entry_drop(_py, entry);
            }
        }
    }
    {
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        for (_slot, entries) in guard.drain() {
            for entry in entries {
                weakset_entry_drop(_py, entry);
            }
        }
    }
}

fn unregister_weakref(_py: &PyToken<'_>, weak_ptr: *mut u8) -> Option<WeakRefEntry> {
    let weak_slot = PtrSlot(weak_ptr);
    let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
    let entry = registry.by_ref.remove(&weak_slot);
    if let Some(entry) = entry.as_ref() {
        if !entry.target.0.is_null() {
            if let Some(list) = registry.by_target.get_mut(&entry.target) {
                list.retain(|slot| *slot != weak_slot);
                if list.is_empty() {
                    registry.by_target.remove(&entry.target);
                }
            }
        }
    }
    entry
}

fn weakref_snapshot_for_target(_py: &PyToken<'_>, target_ptr: *mut u8) -> Vec<(u64, u64)> {
    if target_ptr.is_null() {
        return Vec::new();
    }
    let target_addr = target_ptr.expose_provenance() as u64;
    if resolve_ptr(target_addr).is_none() {
        return Vec::new();
    }
    let target_slot = PtrSlot(target_ptr);
    let registry = runtime_state(_py).weakrefs.lock().unwrap();
    let Some(ref_slots) = registry.by_target.get(&target_slot) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, u64)> = Vec::new();
    for weak_slot in ref_slots {
        let weak_ptr = weak_slot.0;
        if weak_ptr.is_null() {
            continue;
        }
        let weak_addr = weak_ptr.expose_provenance() as u64;
        if resolve_ptr(weak_addr).is_none() {
            continue;
        }
        let Some(entry) = registry.by_ref.get(weak_slot) else {
            continue;
        };
        if entry.target != target_slot || entry.target.0.is_null() {
            continue;
        }
        out.push((MoltObject::from_ptr(weak_ptr).bits(), entry.callback_bits));
    }
    out
}

fn target_bound_in_module_globals(_py: &PyToken<'_>, target_ptr: *mut u8) -> bool {
    let modules: Vec<u64> = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.values().copied().collect()
    };
    for module_bits in modules {
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            continue;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            continue;
        }
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            continue;
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            continue;
        }
        let order = unsafe { dict_order(dict_ptr) };
        for pair in order.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            if let Some(ptr) = obj_from_bits(pair[1]).as_ptr() {
                if ptr == target_ptr {
                    return true;
                }
            }
        }
    }
    false
}

fn expect_obj_ptr(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<*mut u8, u64> {
    obj_from_bits(bits)
        .as_ptr()
        .ok_or_else(|| raise_exception::<u64>(_py, "TypeError", label))
}

fn weakkey_entry_drop(_py: &PyToken<'_>, entry: WeakKeyDictEntry) {
    dec_ref_bits(_py, entry.key_ref_bits);
    dec_ref_bits(_py, entry.value_bits);
}

fn weakvalue_entry_drop(_py: &PyToken<'_>, entry: WeakValueDictEntry) {
    dec_ref_bits(_py, entry.key_bits);
    dec_ref_bits(_py, entry.value_ref_bits);
}

fn weakset_entry_drop(_py: &PyToken<'_>, entry: WeakSetEntry) {
    dec_ref_bits(_py, entry.item_ref_bits);
}

fn ptr_slot_live(slot: PtrSlot) -> bool {
    if slot.0.is_null() {
        return false;
    }
    resolve_ptr(slot.0.expose_provenance() as u64).is_some()
}

fn prune_weakkeydict_slots(_py: &PyToken<'_>) {
    let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
    let dead: Vec<PtrSlot> = guard
        .keys()
        .copied()
        .filter(|slot| !ptr_slot_live(*slot))
        .collect();
    for slot in dead {
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakkey_entry_drop(_py, entry);
            }
        }
    }
}

fn prune_weakvaluedict_slots(_py: &PyToken<'_>) {
    let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
    let dead: Vec<PtrSlot> = guard
        .keys()
        .copied()
        .filter(|slot| !ptr_slot_live(*slot))
        .collect();
    for slot in dead {
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakvalue_entry_drop(_py, entry);
            }
        }
    }
}

fn prune_weakset_slots(_py: &PyToken<'_>) {
    let mut guard = runtime_state(_py).weaksets.lock().unwrap();
    let dead: Vec<PtrSlot> = guard
        .keys()
        .copied()
        .filter(|slot| !ptr_slot_live(*slot))
        .collect();
    for slot in dead {
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakset_entry_drop(_py, entry);
            }
        }
    }
}

fn weakref_target_bits(_py: &PyToken<'_>, weak_bits: u64) -> Result<Option<u64>, u64> {
    let target_bits = molt_weakref_peek(weak_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(target_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(target_bits))
}

fn py_eq_checked(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Result<bool, u64> {
    let eq_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(is_truthy(_py, obj_from_bits(eq_bits)))
}

fn weakkeydict_find_index(
    _py: &PyToken<'_>,
    entries: &mut Vec<WeakKeyDictEntry>,
    key_bits: u64,
    _key_hash: u64,
) -> Result<Option<usize>, u64> {
    let mut idx = 0usize;
    while idx < entries.len() {
        let Some(target_bits) = weakref_target_bits(_py, entries[idx].key_ref_bits)? else {
            let removed = entries.remove(idx);
            weakkey_entry_drop(_py, removed);
            continue;
        };
        if py_eq_checked(_py, target_bits, key_bits)? {
            return Ok(Some(idx));
        }
        idx += 1;
    }
    Ok(None)
}

fn weakvaluedict_find_index(
    _py: &PyToken<'_>,
    entries: &mut Vec<WeakValueDictEntry>,
    key_bits: u64,
    _key_hash: u64,
) -> Result<Option<usize>, u64> {
    let mut idx = 0usize;
    while idx < entries.len() {
        let Some(_) = weakref_target_bits(_py, entries[idx].value_ref_bits)? else {
            let removed = entries.remove(idx);
            weakvalue_entry_drop(_py, removed);
            continue;
        };
        if py_eq_checked(_py, entries[idx].key_bits, key_bits)? {
            return Ok(Some(idx));
        }
        idx += 1;
    }
    Ok(None)
}

fn weakset_find_index(
    _py: &PyToken<'_>,
    entries: &mut Vec<WeakSetEntry>,
    item_bits: u64,
    _item_hash: u64,
) -> Result<Option<usize>, u64> {
    let mut idx = 0usize;
    while idx < entries.len() {
        let Some(target_bits) = weakref_target_bits(_py, entries[idx].item_ref_bits)? else {
            let removed = entries.remove(idx);
            weakset_entry_drop(_py, removed);
            continue;
        };
        if py_eq_checked(_py, target_bits, item_bits)? {
            return Ok(Some(idx));
        }
        idx += 1;
    }
    Ok(None)
}

#[no_mangle]
pub extern "C" fn molt_weakref_find_nocallback(target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        for (weak_bits, callback_bits) in weakref_snapshot_for_target(_py, target_ptr) {
            if obj_from_bits(callback_bits).is_none() {
                return weak_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_refs(target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let refs = weakref_snapshot_for_target(_py, target_ptr);
        let bits: Vec<u64> = refs.into_iter().map(|(weak_bits, _)| weak_bits).collect();
        let ptr = alloc_list(_py, bits.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_count(target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            return int_bits_from_i64(_py, 0);
        };
        let count = weakref_snapshot_for_target(_py, target_ptr).len();
        int_bits_from_i64(_py, count as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_register(
    weak_bits: u64,
    target_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            let type_label = type_name(_py, obj_from_bits(target_bits)).into_owned();
            let msg = format!("cannot create weak reference to '{type_label}' object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if !obj_from_bits(callback_bits).is_none() {
            let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(callback_bits)));
            if !callable_ok {
                return raise_exception::<_>(_py, "TypeError", "weakref callback must be callable");
            }
        }
        if let Some(entry) = unregister_weakref(_py, weak_ptr) {
            if !obj_from_bits(entry.callback_bits).is_none() {
                dec_ref_bits(_py, entry.callback_bits);
            }
        }
        if !obj_from_bits(callback_bits).is_none() {
            inc_ref_bits(_py, callback_bits);
        }
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        let weak_slot = PtrSlot(weak_ptr);
        let target_slot = PtrSlot(target_ptr);
        registry.by_ref.insert(
            weak_slot,
            WeakRefEntry {
                target: target_slot,
                callback_bits,
            },
        );
        registry
            .by_target
            .entry(target_slot)
            .or_default()
            .push(weak_slot);
        weak_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_get(weak_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let weak_slot = PtrSlot(weak_ptr);
        let target_ptr = {
            let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
            let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                return MoltObject::none().bits();
            };
            if entry.target.0.is_null() {
                return MoltObject::none().bits();
            }
            let resolved_target = entry.target.0;
            let mut target_ptr = resolved_target;
            let mut prune_target = None;
            let addr = resolved_target.expose_provenance() as u64;
            if resolve_ptr(addr).is_none() {
                // Target is gone but no explicit clear path ran; mark dead and
                // drop callback handle so it cannot fire re-entrantly from lookups.
                entry.target = PtrSlot(ptr::null_mut());
                entry.callback_bits = MoltObject::none().bits();
                prune_target = Some(PtrSlot(resolved_target));
                target_ptr = ptr::null_mut();
            } else {
                let rc = unsafe {
                    let header_ptr = header_from_obj_ptr(resolved_target);
                    (*header_ptr).ref_count.load(AtomicOrdering::Acquire)
                };
                // Module-frame lowering can retain transient owners after names are dropped.
                // Treat small non-module-bound targets as dead to match CPython weakref timing.
                if rc <= 2 && !target_bound_in_module_globals(_py, resolved_target) {
                    entry.target = PtrSlot(ptr::null_mut());
                    entry.callback_bits = MoltObject::none().bits();
                    prune_target = Some(PtrSlot(resolved_target));
                    target_ptr = ptr::null_mut();
                }
            }
            if let Some(target_slot) = prune_target {
                if let Some(list) = registry.by_target.get_mut(&target_slot) {
                    list.retain(|slot| *slot != weak_slot);
                    if list.is_empty() {
                        registry.by_target.remove(&target_slot);
                    }
                }
            }
            target_ptr
        };
        if target_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(target_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_peek(weak_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let weak_slot = PtrSlot(weak_ptr);
        let target_ptr = {
            let registry = runtime_state(_py).weakrefs.lock().unwrap();
            let Some(entry) = registry.by_ref.get(&weak_slot) else {
                return MoltObject::none().bits();
            };
            if entry.target.0.is_null() {
                return MoltObject::none().bits();
            }
            entry.target.0
        };
        let addr = target_ptr.expose_provenance() as u64;
        if resolve_ptr(addr).is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(target_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_drop(weak_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if let Some(entry) = unregister_weakref(_py, weak_ptr) {
            if !obj_from_bits(entry.callback_bits).is_none() {
                dec_ref_bits(_py, entry.callback_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_collect() -> u64 {
    crate::with_gil_entry!(_py, {
        weakref_collect_for_gc(_py);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_finalize_track(finalizer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(finalizer_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref finalize tracker expects callable object",
            );
        }
        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(finalizer_bits)));
        if !callable_ok {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref finalize tracker expects callable object",
            );
        }
        let mut guard = runtime_state(_py).weakref_finalizers.lock().unwrap();
        if !guard.contains(&finalizer_bits) {
            inc_ref_bits(_py, finalizer_bits);
            guard.push(finalizer_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakref_finalize_untrack(finalizer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut guard = runtime_state(_py).weakref_finalizers.lock().unwrap();
        let mut idx = 0usize;
        while idx < guard.len() {
            if guard[idx] == finalizer_bits {
                let bits = guard.remove(idx);
                dec_ref_bits(_py, bits);
            } else {
                idx += 1;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_set(
    dict_bits: u64,
    key_bits: u64,
    key_ref_bits: u64,
    key_hash_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        if let Err(bits) =
            expect_obj_ptr(_py, key_ref_bits, "WeakKeyDictionary keyref expects object")
        {
            return bits;
        }
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let entries = guard.entry(slot).or_default();
        let found = match weakkeydict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        if let Some(idx) = found {
            inc_ref_bits(_py, value_bits);
            let old = std::mem::replace(&mut entries[idx].value_bits, value_bits);
            dec_ref_bits(_py, old);
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, key_ref_bits);
        inc_ref_bits(_py, value_bits);
        entries.push(WeakKeyDictEntry {
            key_ref_bits,
            value_bits,
        });
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_get(dict_bits: u64, key_bits: u64, key_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "weak key not found");
        };
        let found = match weakkeydict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        let Some(idx) = found else {
            return raise_exception::<_>(_py, "KeyError", "weak key not found");
        };
        let out = entries[idx].value_bits;
        inc_ref_bits(_py, out);
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_del(dict_bits: u64, key_bits: u64, key_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "weak key not found");
        };
        let found = match weakkeydict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        let Some(idx) = found else {
            return raise_exception::<_>(_py, "KeyError", "weak key not found");
        };
        let removed = entries.remove(idx);
        weakkey_entry_drop(_py, removed);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_contains(
    dict_bits: u64,
    key_bits: u64,
    key_hash_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return MoltObject::from_bool(false).bits();
        };
        let found = match weakkeydict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(found.is_some()).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_len(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return int_bits_from_i64(_py, 0);
        };
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(_) = (match weakref_target_bits(_py, entries[idx].key_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakkey_entry_drop(_py, removed);
                continue;
            };
            idx += 1;
        }
        int_bits_from_i64(_py, entries.len() as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_items(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let mut out: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(key_obj_bits) = (match weakref_target_bits(_py, entries[idx].key_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakkey_entry_drop(_py, removed);
                continue;
            };
            let tuple_ptr = alloc_tuple(_py, &[key_obj_bits, entries[idx].value_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(tuple_ptr).bits());
            idx += 1;
        }
        let ptr = alloc_list(_py, out.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_keyrefs(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let mut refs: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(_) = (match weakref_target_bits(_py, entries[idx].key_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakkey_entry_drop(_py, removed);
                continue;
            };
            refs.push(entries[idx].key_ref_bits);
            idx += 1;
        }
        let ptr = alloc_list(_py, refs.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_popitem(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
        };
        let idx = 0usize;
        while idx < entries.len() {
            let Some(key_obj_bits) = (match weakref_target_bits(_py, entries[idx].key_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakkey_entry_drop(_py, removed);
                continue;
            };
            let value_bits = entries[idx].value_bits;
            let tuple_ptr = alloc_tuple(_py, &[key_obj_bits, value_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let removed = entries.remove(idx);
            weakkey_entry_drop(_py, removed);
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty")
    })
}

#[no_mangle]
pub extern "C" fn molt_weakkeydict_clear(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakkeydict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakKeyDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakkeydicts.lock().unwrap();
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakkey_entry_drop(_py, entry);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_set(
    dict_bits: u64,
    key_bits: u64,
    key_hash_bits: u64,
    value_ref_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        if let Err(bits) = expect_obj_ptr(
            _py,
            value_ref_bits,
            "WeakValueDictionary value ref expects object",
        ) {
            return bits;
        }
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let entries = guard.entry(slot).or_default();
        let found = match weakvaluedict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        if let Some(idx) = found {
            inc_ref_bits(_py, value_ref_bits);
            let old = std::mem::replace(&mut entries[idx].value_ref_bits, value_ref_bits);
            dec_ref_bits(_py, old);
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, value_ref_bits);
        entries.push(WeakValueDictEntry {
            key_bits,
            value_ref_bits,
        });
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_get(dict_bits: u64, key_bits: u64, key_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "weak value key not found");
        };
        let found = match weakvaluedict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        let Some(idx) = found else {
            return raise_exception::<_>(_py, "KeyError", "weak value key not found");
        };
        let Some(value_bits) = (match weakref_target_bits(_py, entries[idx].value_ref_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        }) else {
            let removed = entries.remove(idx);
            weakvalue_entry_drop(_py, removed);
            return raise_exception::<_>(_py, "KeyError", "weak value key not found");
        };
        inc_ref_bits(_py, value_bits);
        value_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_del(dict_bits: u64, key_bits: u64, key_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "weak value key not found");
        };
        let found = match weakvaluedict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        let Some(idx) = found else {
            return raise_exception::<_>(_py, "KeyError", "weak value key not found");
        };
        let removed = entries.remove(idx);
        weakvalue_entry_drop(_py, removed);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_contains(
    dict_bits: u64,
    key_bits: u64,
    key_hash_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let key_hash = key_hash_bits;
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return MoltObject::from_bool(false).bits();
        };
        let found = match weakvaluedict_find_index(_py, entries, key_bits, key_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(found.is_some()).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_len(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return int_bits_from_i64(_py, 0);
        };
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(_) = (match weakref_target_bits(_py, entries[idx].value_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakvalue_entry_drop(_py, removed);
                continue;
            };
            idx += 1;
        }
        int_bits_from_i64(_py, entries.len() as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_items(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let mut out: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(value_bits) = (match weakref_target_bits(_py, entries[idx].value_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakvalue_entry_drop(_py, removed);
                continue;
            };
            let tuple_ptr = alloc_tuple(_py, &[entries[idx].key_bits, value_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(tuple_ptr).bits());
            idx += 1;
        }
        let ptr = alloc_list(_py, out.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_valuerefs(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let mut refs: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(_) = (match weakref_target_bits(_py, entries[idx].value_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakvalue_entry_drop(_py, removed);
                continue;
            };
            refs.push(entries[idx].value_ref_bits);
            idx += 1;
        }
        let ptr = alloc_list(_py, refs.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_popitem(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
        };
        let idx = 0usize;
        while idx < entries.len() {
            let Some(value_bits) = (match weakref_target_bits(_py, entries[idx].value_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakvalue_entry_drop(_py, removed);
                continue;
            };
            let tuple_ptr = alloc_tuple(_py, &[entries[idx].key_bits, value_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let removed = entries.remove(idx);
            weakvalue_entry_drop(_py, removed);
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty")
    })
}

#[no_mangle]
pub extern "C" fn molt_weakvaluedict_clear(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakvaluedict_slots(_py);
        let dict_ptr = match expect_obj_ptr(_py, dict_bits, "WeakValueDictionary expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(dict_ptr);
        let mut guard = runtime_state(_py).weakvaluedicts.lock().unwrap();
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakvalue_entry_drop(_py, entry);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_add(
    set_bits: u64,
    item_bits: u64,
    item_ref_bits: u64,
    item_hash_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        if let Err(bits) = expect_obj_ptr(_py, item_ref_bits, "WeakSet ref expects object") {
            return bits;
        }
        let item_hash = item_hash_bits;
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let entries = guard.entry(slot).or_default();
        let found = match weakset_find_index(_py, entries, item_bits, item_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        if found.is_some() {
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, item_ref_bits);
        entries.push(WeakSetEntry { item_ref_bits });
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_discard(set_bits: u64, item_bits: u64, item_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let item_hash = item_hash_bits;
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return MoltObject::none().bits();
        };
        if let Some(idx) = match weakset_find_index(_py, entries, item_bits, item_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        } {
            let removed = entries.remove(idx);
            weakset_entry_drop(_py, removed);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_remove(set_bits: u64, item_bits: u64, item_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let item_hash = item_hash_bits;
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "weakset remove missing item");
        };
        let Some(idx) = (match weakset_find_index(_py, entries, item_bits, item_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        }) else {
            return raise_exception::<_>(_py, "KeyError", "weakset remove missing item");
        };
        let removed = entries.remove(idx);
        weakset_entry_drop(_py, removed);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_pop(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return raise_exception::<_>(_py, "KeyError", "pop from empty WeakSet");
        };
        let idx = 0usize;
        while idx < entries.len() {
            let Some(item_bits) = (match weakref_target_bits(_py, entries[idx].item_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakset_entry_drop(_py, removed);
                continue;
            };
            inc_ref_bits(_py, item_bits);
            let removed = entries.remove(idx);
            weakset_entry_drop(_py, removed);
            return item_bits;
        }
        raise_exception::<_>(_py, "KeyError", "pop from empty WeakSet")
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_contains(set_bits: u64, item_bits: u64, item_hash_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let item_hash = item_hash_bits;
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return MoltObject::from_bool(false).bits();
        };
        let found = match weakset_find_index(_py, entries, item_bits, item_hash) {
            Ok(found) => found,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(found.is_some()).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_len(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            return int_bits_from_i64(_py, 0);
        };
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(_) = (match weakref_target_bits(_py, entries[idx].item_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakset_entry_drop(_py, removed);
                continue;
            };
            idx += 1;
        }
        int_bits_from_i64(_py, entries.len() as i64)
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_items(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        let Some(entries) = guard.get_mut(&slot) else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let mut out: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < entries.len() {
            let Some(item_bits) = (match weakref_target_bits(_py, entries[idx].item_ref_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }) else {
                let removed = entries.remove(idx);
                weakset_entry_drop(_py, removed);
                continue;
            };
            out.push(item_bits);
            idx += 1;
        }
        let ptr = alloc_list(_py, out.as_slice());
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_weakset_clear(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        prune_weakset_slots(_py);
        let set_ptr = match expect_obj_ptr(_py, set_bits, "WeakSet expects object") {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let slot = PtrSlot(set_ptr);
        let mut guard = runtime_state(_py).weaksets.lock().unwrap();
        if let Some(entries) = guard.remove(&slot) {
            for entry in entries {
                weakset_entry_drop(_py, entry);
            }
        }
        MoltObject::none().bits()
    })
}
