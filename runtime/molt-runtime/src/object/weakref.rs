use crate::state::runtime_state::WeakRefEntry;
use crate::{
    call_callable1, clear_exception, dec_ref_bits, exception_pending, inc_ref_bits, is_truthy,
    obj_from_bits,
};
use crate::{
    molt_is_callable, raise_exception, runtime_state, type_name, MoltObject, PtrSlot, PyToken,
};
use std::ptr;

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
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(entry) = registry.by_ref.get(&PtrSlot(weak_ptr)) else {
            return MoltObject::none().bits();
        };
        if entry.target.0.is_null() {
            return MoltObject::none().bits();
        }
        let bits = MoltObject::from_ptr(entry.target.0).bits();
        inc_ref_bits(_py, bits);
        bits
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
