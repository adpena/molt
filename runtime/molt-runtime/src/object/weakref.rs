use crate::state::runtime_state::WeakRefEntry;
use crate::{
    call_callable1, clear_exception, dec_ref_bits, dict_order, exception_pending,
    header_from_obj_ptr, inc_ref_bits, is_truthy, module_dict_bits, obj_from_bits, object_type_id,
    resolve_ptr, TYPE_ID_DICT, TYPE_ID_MODULE,
};
use crate::{
    molt_is_callable, raise_exception, runtime_state, type_name, MoltObject, PtrSlot, PyToken,
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
        for ptr in targets {
            weakref_clear_for_ptr(_py, ptr);
        }
        MoltObject::none().bits()
    })
}
