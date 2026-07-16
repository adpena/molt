use crate::state::runtime_state::{WeakRefEntry, WeakRefRegistry};
use crate::{
    MoltObject, PtrSlot, PyToken, attr_name_bits_from_bytes, molt_get_attr_name, molt_is_callable,
    raise_exception, runtime_state, type_name,
};
use crate::{
    alloc_list, call_callable0, call_callable1, dec_ref_bits, exception_pending,
    header_from_obj_ptr, inc_ref_bits, int_bits_from_i64, is_truthy, obj_from_bits,
};
use std::ptr;
use std::sync::atomic::Ordering as AtomicOrdering;

use super::weak_container::{WeakEntryId, weakcontainer_target_dead};

#[inline]
fn builtin_type_id_supports_weakrefs(type_id: u32) -> bool {
    matches!(
        type_id,
        crate::TYPE_ID_FUNCTION
            | crate::TYPE_ID_MODULE
            | crate::TYPE_ID_TYPE
            | crate::TYPE_ID_GENERATOR
            | crate::TYPE_ID_ASYNC_GENERATOR
            | crate::TYPE_ID_SET
            | crate::TYPE_ID_FROZENSET
            | crate::TYPE_ID_CODE
    )
}

#[inline]
fn internal_type_id_forbids_weakrefs(type_id: u32) -> bool {
    matches!(
        type_id,
        crate::TYPE_ID_LIST_BUILDER
            | crate::TYPE_ID_DICT_BUILDER
            | crate::TYPE_ID_SET_BUILDER
            | crate::TYPE_ID_CALLARGS
            | crate::TYPE_ID_NATIVE_HANDLE
            | crate::TYPE_ID_TRACEBACK_PAYLOAD
            | crate::TYPE_ID_ITER
            | crate::TYPE_ID_GLOB_ITER
            | crate::TYPE_ID_FOREIGN
            | crate::TYPE_ID_WEAK_CONTAINER_STATE
    )
}

/// Single fail-closed weakrefability authority for runtime registration.
/// Internal/builders and unlisted builtins are rejected even when heap-backed.
pub(crate) fn object_supports_weakrefs(_py: &PyToken<'_>, target_bits: u64) -> bool {
    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
        return false;
    };
    let type_id = unsafe { crate::object_type_id(target_ptr) };
    if internal_type_id_forbids_weakrefs(type_id) {
        return false;
    }
    let class_bits = crate::type_of_bits(_py, target_bits);
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return builtin_type_id_supports_weakrefs(type_id);
    };
    if unsafe { crate::object_type_id(class_ptr) } == crate::TYPE_ID_TYPE
        && !crate::is_builtin_class_bits(_py, class_bits)
    {
        return unsafe { crate::builtins::attr::class_slots_info(_py, class_ptr) }
            .is_none_or(|info| info.allows_weakref);
    }
    if type_id == crate::TYPE_ID_BOUND_METHOD {
        let func_bits = unsafe { crate::bound_method_func_bits(target_ptr) };
        let func_class = crate::type_of_bits(_py, func_bits);
        return func_class
            != crate::builtins::classes::builtin_classes(_py).builtin_function_or_method;
    }
    builtin_type_id_supports_weakrefs(type_id)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WeakContainerCookie {
    pub(crate) state_bits: u64,
    pub(crate) entry: WeakEntryId,
}

struct PendingWeakDeath {
    weak_bits: Option<u64>,
    callback_bits: Option<u64>,
    cookie: Option<WeakContainerCookie>,
}

enum WeakRefRegisterFailure {
    AlreadyRegistered,
    OutOfMemory,
}

/// Current object storage does not yet have a dedicated weakref type id. Until
/// that RC/GC authority exists, registration is gated by the exact stdlib
/// ReferenceType class retained during controlled module initialization. A
/// native TYPE_ID_WEAKREF should replace this cached-class fact in the next
/// ownership arc.
fn object_is_weakref_slot(_py: &PyToken<'_>, weak_ptr: *mut u8) -> bool {
    if unsafe { crate::object_type_id(weak_ptr) } != crate::TYPE_ID_OBJECT {
        return false;
    }
    let reference_type_bits = runtime_state(_py)
        .weakref_reference_type
        .load(AtomicOrdering::Acquire);
    if reference_type_bits == 0 {
        return false;
    }
    let class_bits = crate::type_of_bits(_py, MoltObject::from_ptr(weak_ptr).bits());
    crate::issubclass_bits(class_bits, reference_type_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_bind_reference_type(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "ReferenceType binding must be a type");
        };
        if unsafe { crate::object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
            return raise_exception::<_>(_py, "TypeError", "ReferenceType binding must be a type");
        }
        inc_ref_bits(_py, class_bits);
        match runtime_state(_py).weakref_reference_type.compare_exchange(
            0,
            class_bits,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        ) {
            Ok(_) => MoltObject::none().bits(),
            Err(current) if current == class_bits => {
                dec_ref_bits(_py, class_bits);
                MoltObject::none().bits()
            }
            Err(_) => {
                dec_ref_bits(_py, class_bits);
                raise_exception::<_>(_py, "RuntimeError", "ReferenceType is already bound")
            }
        }
    })
}

pub(crate) fn weakref_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::runtime_state::RuntimeState,
) {
    let class_bits = state.weakref_reference_type.swap(0, AtomicOrdering::AcqRel);
    if class_bits != 0 {
        dec_ref_bits(_py, class_bits);
    }
}

/// Retain a cookie's state object only if its ordinary owner count is still
/// live. Registry custody guarantees state drop cannot detach the cookie and
/// free storage while this inspects its header. The successful path releases
/// registry custody before taking the state lock; state drop likewise releases
/// the state lock before detaching cookies, so the order cannot deadlock.
fn try_pin_cookie_state(cookie: WeakContainerCookie) -> bool {
    let Some(state_ptr) = obj_from_bits(cookie.state_bits).as_ptr() else {
        return false;
    };
    let header = unsafe { header_from_obj_ptr(state_ptr) };
    unsafe {
        if (*header).type_id != crate::TYPE_ID_WEAK_CONTAINER_STATE
            || ((*header).load_flags() & super::HEADER_FLAG_DEALLOCATING) != 0
        {
            return false;
        }
        let mut current = (*header).ref_count.load(AtomicOrdering::Acquire);
        loop {
            if current == 0 || current == u32::MAX {
                return false;
            }
            match (*header).ref_count.compare_exchange_weak(
                current,
                current + 1,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => {
                    debug_assert_eq!(
                        (*header).load_flags() & super::HEADER_FLAG_DEALLOCATING,
                        0,
                        "state entered terminal death after a successful live retain"
                    );
                    return true;
                }
                Err(observed) => current = observed,
            }
        }
    }
}

fn run_pending_weak_death(_py: &PyToken<'_>, death: PendingWeakDeath) {
    if let Some(cookie) = death.cookie {
        // The registry lock pinned state_bits before publishing this work.
        weakcontainer_target_dead(_py, cookie);
        dec_ref_bits(_py, cookie.state_bits);
    }
    if let (Some(weak_bits), Some(cb_bits)) = (death.weak_bits, death.callback_bits) {
        let res_bits = crate::builtins::exceptions::run_unraisable_with_policy(
            _py,
            || weakref_unraisable_policy(_py, cb_bits),
            || unsafe { call_callable1(_py, cb_bits, weak_bits) },
        );
        if !obj_from_bits(res_bits).is_none() {
            dec_ref_bits(_py, res_bits);
        }
        dec_ref_bits(_py, cb_bits);
        dec_ref_bits(_py, weak_bits);
    }
}

fn run_pending_weak_deaths(_py: &PyToken<'_>, deaths: Vec<PendingWeakDeath>) {
    for death in deaths {
        run_pending_weak_death(_py, death);
    }
}

fn weakref_clear_for_ptr_noqueue(_py: &PyToken<'_>, target_slot: PtrSlot) {
    let list = {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(list) = registry.by_target.remove(&target_slot) else {
            return;
        };
        for weak_slot in &list {
            if let Some(entry) = registry.by_ref.get_mut(weak_slot) {
                entry.target = PtrSlot(ptr::null_mut());
            }
        }
        list
    };
    for weak_slot in list.into_iter().rev() {
        let death = {
            let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
            let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                continue;
            };
            let cookie = entry
                .container_cookie
                .take()
                .filter(|cookie| try_pin_cookie_state(*cookie));
            let callback_bits = (!obj_from_bits(entry.callback_bits).is_none()).then(|| {
                let callback_bits = entry.callback_bits;
                entry.callback_bits = MoltObject::none().bits();
                callback_bits
            });
            let weak_bits = callback_bits.map(|_| {
                let bits = MoltObject::from_ptr(weak_slot.0).bits();
                inc_ref_bits(_py, bits);
                bits
            });
            PendingWeakDeath {
                weak_bits,
                callback_bits,
                cookie,
            }
        };
        run_pending_weak_death(_py, death);
    }
}

fn weakref_unraisable_policy(_py: &PyToken<'_>, callback_bits: u64) -> (u64, Option<String>) {
    if crate::object::ops_sys::runtime_target_minor(_py) >= 14 {
        let rendered = crate::builtins::exceptions::unraisable_context_repr(_py, callback_bits);
        (
            MoltObject::none().bits(),
            Some(format!(
                "Exception ignored while calling weakref callback {rendered}"
            )),
        )
    } else {
        (callback_bits, None)
    }
}

pub(crate) fn weakref_clear_for_ptr(_py: &PyToken<'_>, target_ptr: *mut u8) {
    if target_ptr.is_null() {
        return;
    }
    let target_slot = PtrSlot(target_ptr);
    let capacity = runtime_state(_py)
        .weakrefs
        .lock()
        .unwrap()
        .by_target
        .get(&target_slot)
        .map_or(0, Vec::len);
    let mut deaths: Vec<PendingWeakDeath> = Vec::new();
    if deaths.try_reserve_exact(capacity).is_err() {
        weakref_clear_for_ptr_noqueue(_py, target_slot);
        return;
    }
    {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(list) = registry.by_target.remove(&target_slot) else {
            return;
        };
        // CPython invokes callbacks newest registration first.
        for weak_slot in list.into_iter().rev() {
            if weak_slot.0.is_null() {
                continue;
            }
            let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                continue;
            };
            entry.target = PtrSlot(ptr::null_mut());
            let cookie = entry.container_cookie.take();
            let cookie = cookie.filter(|cookie| try_pin_cookie_state(*cookie));
            let cb_bits = entry.callback_bits;
            if !obj_from_bits(cb_bits).is_none() {
                // CPython runs weakref callbacks at most once.
                entry.callback_bits = MoltObject::none().bits();
                let weak_bits = MoltObject::from_ptr(weak_slot.0).bits();
                // Transfer the registration's owned callback edge into the
                // invocation queue. Only the weakref argument needs a new pin.
                inc_ref_bits(_py, weak_bits);
                deaths.push(PendingWeakDeath {
                    weak_bits: Some(weak_bits),
                    callback_bits: Some(cb_bits),
                    cookie,
                });
            } else if cookie.is_some() {
                deaths.push(PendingWeakDeath {
                    weak_bits: None,
                    callback_bits: None,
                    cookie,
                });
            }
        }
    }
    run_pending_weak_deaths(_py, deaths);
}

/// CPython `handle_weakrefs` for cyclic collection: clear every weakref into the
/// unreachable set before running any surviving callback.
pub(crate) fn weakref_handle_cycle_unreachable(
    _py: &PyToken<'_>,
    unreachable: &[*mut u8],
    is_collecting: impl Fn(*mut u8) -> bool,
) {
    let capacity = {
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        unreachable
            .iter()
            .map(|ptr| registry.by_target.get(&PtrSlot(*ptr)).map_or(0, Vec::len))
            .sum()
    };
    let mut deaths: Vec<PendingWeakDeath> = Vec::new();
    let mut dropped_callbacks: Vec<u64> = Vec::new();
    if deaths.try_reserve_exact(capacity).is_err()
        || dropped_callbacks.try_reserve_exact(capacity).is_err()
    {
        weakref_handle_cycle_unreachable_noqueue(_py, unreachable, &is_collecting);
        return;
    }
    {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        for &target_ptr in unreachable {
            if target_ptr.is_null() {
                continue;
            }
            let target_slot = PtrSlot(target_ptr);
            let Some(list) = registry.by_target.remove(&target_slot) else {
                continue;
            };
            // CPython invokes callbacks newest registration first.
            for weak_slot in list.into_iter().rev() {
                if weak_slot.0.is_null() {
                    continue;
                }
                let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                    continue;
                };
                entry.target = PtrSlot(ptr::null_mut());
                let cookie = entry
                    .container_cookie
                    .take()
                    .filter(|cookie| try_pin_cookie_state(*cookie));
                let cb_bits = entry.callback_bits;
                if obj_from_bits(cb_bits).is_none() {
                    if cookie.is_some() {
                        deaths.push(PendingWeakDeath {
                            weak_bits: None,
                            callback_bits: None,
                            cookie,
                        });
                    }
                    continue;
                }
                entry.callback_bits = MoltObject::none().bits();
                if is_collecting(weak_slot.0) {
                    dropped_callbacks.push(cb_bits);
                    if cookie.is_some() {
                        deaths.push(PendingWeakDeath {
                            weak_bits: None,
                            callback_bits: None,
                            cookie,
                        });
                    }
                    continue;
                }
                let weak_bits = MoltObject::from_ptr(weak_slot.0).bits();
                // Transfer the registration's owned callback edge into the
                // invocation queue. Only the weakref argument needs a new pin.
                inc_ref_bits(_py, weak_bits);
                deaths.push(PendingWeakDeath {
                    weak_bits: Some(weak_bits),
                    callback_bits: Some(cb_bits),
                    cookie,
                });
            }
        }
    }
    for cb_bits in dropped_callbacks {
        dec_ref_bits(_py, cb_bits);
    }
    run_pending_weak_deaths(_py, deaths);
}

fn weakref_handle_cycle_unreachable_noqueue(
    _py: &PyToken<'_>,
    unreachable: &[*mut u8],
    is_collecting: &impl Fn(*mut u8) -> bool,
) {
    // Pass 1 clears every target before any callback, preserving CPython's
    // whole-unreachable-set ordering without allocating a side queue.
    {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        for &target_ptr in unreachable {
            let target_slot = PtrSlot(target_ptr);
            let Some(list) = registry.by_target.remove(&target_slot) else {
                continue;
            };
            for weak_slot in &list {
                if let Some(entry) = registry.by_ref.get_mut(weak_slot) {
                    entry.target = PtrSlot(ptr::null_mut());
                }
            }
            registry.by_target.insert(target_slot, list);
        }
    }
    // Pass 2 consumes the already-cleared registrations one at a time. The
    // removed target Vec is the worklist, so the OOM path stays allocation-free.
    for &target_ptr in unreachable {
        let target_slot = PtrSlot(target_ptr);
        let list = runtime_state(_py)
            .weakrefs
            .lock()
            .unwrap()
            .by_target
            .remove(&target_slot);
        let Some(list) = list else { continue };
        for weak_slot in list.into_iter().rev() {
            let collecting = is_collecting(weak_slot.0);
            let death = {
                let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
                let Some(entry) = registry.by_ref.get_mut(&weak_slot) else {
                    continue;
                };
                let cookie = entry
                    .container_cookie
                    .take()
                    .filter(|cookie| try_pin_cookie_state(*cookie));
                let callback_bits = (!obj_from_bits(entry.callback_bits).is_none()).then(|| {
                    let bits = entry.callback_bits;
                    entry.callback_bits = MoltObject::none().bits();
                    bits
                });
                let weak_bits = if collecting || callback_bits.is_none() {
                    None
                } else {
                    let bits = MoltObject::from_ptr(weak_slot.0).bits();
                    inc_ref_bits(_py, bits);
                    Some(bits)
                };
                PendingWeakDeath {
                    weak_bits,
                    callback_bits,
                    cookie,
                }
            };
            if collecting {
                if let Some(callback_bits) = death.callback_bits {
                    dec_ref_bits(_py, callback_bits);
                }
                if let Some(cookie) = death.cookie {
                    weakcontainer_target_dead(_py, cookie);
                    dec_ref_bits(_py, cookie.state_bits);
                }
            } else {
                run_pending_weak_death(_py, death);
            }
        }
    }
}

pub(crate) fn weakref_run_atexit_finalizers(_py: &PyToken<'_>) {
    while let Some(finalizer_bits) = crate::builtins::atexit::pop_weakref_finalizer(_py) {
        let should_run = crate::builtins::exceptions::run_unraisable_with_policy(
            _py,
            || weakref_unraisable_policy(_py, finalizer_bits),
            || weakref_finalizer_should_run_atexit(_py, finalizer_bits),
        );
        if should_run {
            let res_bits = crate::builtins::exceptions::run_unraisable_with_policy(
                _py,
                || weakref_unraisable_policy(_py, finalizer_bits),
                || unsafe { call_callable0(_py, finalizer_bits) },
            );
            if !obj_from_bits(res_bits).is_none() {
                dec_ref_bits(_py, res_bits);
            }
        }
        dec_ref_bits(_py, finalizer_bits);
    }
}

fn weakref_finalizer_should_run_atexit(_py: &PyToken<'_>, finalizer_bits: u64) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"atexit") else {
        return true;
    };
    let value_bits = molt_get_attr_name(finalizer_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return true;
    }
    let should_run = is_truthy(_py, obj_from_bits(value_bits));
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    should_run
}

fn unregister_weakref(_py: &PyToken<'_>, weak_ptr: *mut u8) -> Option<WeakRefEntry> {
    let weak_slot = PtrSlot(weak_ptr);
    let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
    let mut entry = registry.by_ref.remove(&weak_slot);
    if let Some(entry) = entry.as_mut()
        && let Some(cookie) = entry.container_cookie
        && !try_pin_cookie_state(cookie)
    {
        entry.container_cookie = None;
    }
    if let Some(entry) = entry.as_ref()
        && !entry.target.0.is_null()
        && let Some(list) = registry.by_target.get_mut(&entry.target)
    {
        list.retain(|slot| *slot != weak_slot);
        if list.is_empty() {
            registry.by_target.remove(&entry.target);
        }
    }
    entry
}

pub(crate) fn weakref_object_detach(_py: &PyToken<'_>, weak_ptr: *mut u8) -> Option<WeakRefEntry> {
    unregister_weakref(_py, weak_ptr)
}

pub(crate) fn weakref_object_release(_py: &PyToken<'_>, entry: Option<WeakRefEntry>) {
    if let Some(entry) = entry {
        if let Some(cookie) = entry.container_cookie {
            weakcontainer_target_dead(_py, cookie);
            dec_ref_bits(_py, cookie.state_bits);
        }
        if !obj_from_bits(entry.callback_bits).is_none() {
            dec_ref_bits(_py, entry.callback_bits);
        }
    }
}

pub(crate) fn weakref_object_callback_bits(_py: &PyToken<'_>, weak_ptr: *mut u8) -> Option<u64> {
    let weak_slot = PtrSlot(weak_ptr);
    let registry = runtime_state(_py).weakrefs.lock().unwrap();
    let bits = registry.by_ref.get(&weak_slot)?.callback_bits;
    if obj_from_bits(bits).is_none() {
        return None;
    }
    inc_ref_bits(_py, bits);
    Some(bits)
}

fn weakref_resolve_target_ptr(registry: &WeakRefRegistry, weak_slot: PtrSlot) -> Option<*mut u8> {
    let entry = registry.by_ref.get(&weak_slot)?;
    (!entry.target.0.is_null()).then_some(entry.target.0)
}

pub(crate) fn weakref_attach_container_cookie(
    _py: &PyToken<'_>,
    weak_bits: u64,
    cookie: WeakContainerCookie,
) -> bool {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return false;
    };
    let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
    let Some(entry) = registry.by_ref.get_mut(&PtrSlot(weak_ptr)) else {
        return false;
    };
    if entry.target.0.is_null() || entry.container_cookie.is_some() {
        return false;
    }
    entry.container_cookie = Some(cookie);
    true
}

pub(crate) fn weakref_detach_container_cookie(_py: &PyToken<'_>, weak_bits: u64) {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return;
    };
    if let Some(entry) = runtime_state(_py)
        .weakrefs
        .lock()
        .unwrap()
        .by_ref
        .get_mut(&PtrSlot(weak_ptr))
    {
        entry.container_cookie = None;
    }
}

pub(crate) fn weakref_container_cookie(
    _py: &PyToken<'_>,
    weak_bits: u64,
) -> Option<WeakContainerCookie> {
    let weak_ptr = obj_from_bits(weak_bits).as_ptr()?;
    runtime_state(_py)
        .weakrefs
        .lock()
        .unwrap()
        .by_ref
        .get(&PtrSlot(weak_ptr))
        .and_then(|entry| entry.container_cookie)
}

/// Resolve and pin a weak referent while target-link custody is held.
pub(crate) fn weakref_peek_owned(_py: &PyToken<'_>, weak_bits: u64) -> Option<u64> {
    let weak_ptr = obj_from_bits(weak_bits).as_ptr()?;
    let registry = runtime_state(_py).weakrefs.lock().unwrap();
    let target_ptr = weakref_resolve_target_ptr(&registry, PtrSlot(weak_ptr))?;
    let target_bits = MoltObject::from_ptr(target_ptr).bits();
    inc_ref_bits(_py, target_bits);
    Some(target_bits)
}

fn weakref_snapshot_for_target(_py: &PyToken<'_>, target_ptr: *mut u8) -> Result<Vec<u64>, u64> {
    if target_ptr.is_null() {
        return Ok(Vec::new());
    }
    let target_slot = PtrSlot(target_ptr);
    loop {
        let capacity = runtime_state(_py)
            .weakrefs
            .lock()
            .unwrap()
            .by_target
            .get(&target_slot)
            .map_or(0, Vec::len);
        let mut out: Vec<u64> = Vec::new();
        out.try_reserve_exact(capacity).map_err(|_| {
            raise_exception::<u64>(_py, "MemoryError", "weakref snapshot allocation failed")
        })?;
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(ref_slots) = registry.by_target.get(&target_slot) else {
            return Ok(out);
        };
        if ref_slots.len() > out.capacity() {
            continue;
        }
        for weak_slot in ref_slots {
            let weak_ptr = weak_slot.0;
            if weak_ptr.is_null() {
                continue;
            }
            let Some(entry) = registry.by_ref.get(weak_slot) else {
                continue;
            };
            if entry.target != target_slot || entry.target.0.is_null() {
                continue;
            }
            let weak_bits = MoltObject::from_ptr(weak_ptr).bits();
            inc_ref_bits(_py, weak_bits);
            out.push(weak_bits);
        }
        return Ok(out);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_find_nocallback(target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let target_slot = PtrSlot(target_ptr);
        if let Some(ref_slots) = registry.by_target.get(&target_slot) {
            for weak_slot in ref_slots {
                let Some(entry) = registry.by_ref.get(weak_slot) else {
                    continue;
                };
                if entry.target == target_slot && obj_from_bits(entry.callback_bits).is_none() {
                    let weak_bits = MoltObject::from_ptr(weak_slot.0).bits();
                    inc_ref_bits(_py, weak_bits);
                    return weak_bits;
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_refs(target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            let ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(ptr).bits();
        };
        let refs = match weakref_snapshot_for_target(_py, target_ptr) {
            Ok(refs) => refs,
            Err(bits) => return bits,
        };
        let ptr = alloc_list(_py, refs.as_slice());
        for weak_bits in refs {
            dec_ref_bits(_py, weak_bits);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_count(target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            return int_bits_from_i64(_py, 0);
        };
        let target_slot = PtrSlot(target_ptr);
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let count = registry.by_target.get(&target_slot).map_or(0, |refs| {
            refs.iter()
                .filter(|slot| {
                    registry
                        .by_ref
                        .get(slot)
                        .is_some_and(|entry| entry.target == target_slot)
                })
                .count()
        });
        drop(registry);
        int_bits_from_i64(_py, count as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_register(
    weak_bits: u64,
    target_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        if !object_is_weakref_slot(_py, weak_ptr) {
            return raise_exception::<_>(_py, "TypeError", "weakref must be a ReferenceType");
        }
        let weak_slot = PtrSlot(weak_ptr);
        if runtime_state(_py)
            .weakrefs
            .lock()
            .unwrap()
            .by_ref
            .contains_key(&weak_slot)
        {
            return MoltObject::from_bool(false).bits();
        }
        let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
            let type_label = type_name(_py, obj_from_bits(target_bits)).into_owned();
            let msg = format!("cannot create weak reference to '{type_label}' object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let supports_weakrefs = object_supports_weakrefs(_py, target_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !supports_weakrefs {
            let type_label = type_name(_py, obj_from_bits(target_bits)).into_owned();
            let msg = format!("cannot create weak reference to '{type_label}' object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if unsafe { (*header_from_obj_ptr(target_ptr)).load_flags() }
            & super::HEADER_FLAG_DEALLOCATING
            != 0
        {
            return raise_exception::<_>(
                _py,
                "ReferenceError",
                "cannot create weak reference to deallocating object",
            );
        }
        if !obj_from_bits(callback_bits).is_none() {
            let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(callback_bits)));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !callable_ok {
                return raise_exception::<_>(_py, "TypeError", "weakref callback must be callable");
            }
        }
        let target_slot = PtrSlot(target_ptr);
        let mut prepared_target_list = Vec::new();
        if prepared_target_list.try_reserve_exact(1).is_err() {
            return raise_exception::<_>(_py, "MemoryError", "weakref registration failed");
        }
        prepared_target_list.push(weak_slot);
        if !obj_from_bits(callback_bits).is_none() {
            inc_ref_bits(_py, callback_bits);
        }
        let registration = {
            let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
            if registry.by_ref.contains_key(&weak_slot) {
                Err(WeakRefRegisterFailure::AlreadyRegistered)
            } else if registry.by_ref.try_reserve(1).is_err()
                || registry.by_target.try_reserve(1).is_err()
                || registry
                    .by_target
                    .get_mut(&target_slot)
                    .is_some_and(|list| list.try_reserve(1).is_err())
            {
                Err(WeakRefRegisterFailure::OutOfMemory)
            } else {
                registry.by_ref.insert(
                    weak_slot,
                    WeakRefEntry {
                        target: target_slot,
                        callback_bits,
                        container_cookie: None,
                    },
                );
                if let Some(list) = registry.by_target.get_mut(&target_slot) {
                    list.push(weak_slot);
                } else {
                    registry.by_target.insert(target_slot, prepared_target_list);
                }
                Ok(())
            }
        };
        match registration {
            Ok(()) => {}
            Err(failure) => {
                if !obj_from_bits(callback_bits).is_none() {
                    dec_ref_bits(_py, callback_bits);
                }
                return match failure {
                    WeakRefRegisterFailure::AlreadyRegistered => {
                        MoltObject::from_bool(false).bits()
                    }
                    WeakRefRegisterFailure::OutOfMemory => {
                        raise_exception::<_>(_py, "MemoryError", "weakref registration failed")
                    }
                };
            }
        }
        // Publish sticky header facts only after the registry transaction has
        // committed. Failed initialization must not make an arbitrary object
        // enter weakref-specific deallocation.
        unsafe {
            (*header_from_obj_ptr(target_ptr)).fetch_or_flags(super::HEADER_FLAG_HAS_WEAKREF);
            (*header_from_obj_ptr(weak_ptr)).fetch_or_flags(super::HEADER_FLAG_IS_WEAKREF);
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_get(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let weak_slot = PtrSlot(weak_ptr);

        {
            let registry = runtime_state(_py).weakrefs.lock().unwrap();
            let Some(target_ptr) = weakref_resolve_target_ptr(&registry, weak_slot) else {
                return MoltObject::none().bits();
            };
            let target_bits = MoltObject::from_ptr(target_ptr).bits();
            inc_ref_bits(_py, target_bits);
            target_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_callback(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let weak_slot = PtrSlot(weak_ptr);

        {
            let registry = runtime_state(_py).weakrefs.lock().unwrap();
            if weakref_resolve_target_ptr(&registry, weak_slot).is_none() {
                return MoltObject::none().bits();
            }
            let Some(entry) = registry.by_ref.get(&weak_slot) else {
                return MoltObject::none().bits();
            };
            if obj_from_bits(entry.callback_bits).is_none() {
                return MoltObject::none().bits();
            }
            let callback_bits = entry.callback_bits;
            inc_ref_bits(_py, callback_bits);
            callback_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_peek(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
        };
        let weak_slot = PtrSlot(weak_ptr);

        {
            let registry = runtime_state(_py).weakrefs.lock().unwrap();
            let Some(entry) = registry.by_ref.get(&weak_slot) else {
                return MoltObject::none().bits();
            };
            if entry.target.0.is_null() {
                return MoltObject::none().bits();
            }
            let target_bits = MoltObject::from_ptr(entry.target.0).bits();
            inc_ref_bits(_py, target_bits);
            target_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_finalize_track(finalizer_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(finalizer_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref finalize tracker expects callable object",
            );
        }
        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(finalizer_bits)));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !callable_ok {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref finalize tracker expects callable object",
            );
        }
        if let Err(error) = crate::builtins::atexit::weakref_finalizer_track(_py, finalizer_bits) {
            return match error {
                crate::builtins::atexit::ExitRegistryError::OutOfMemory => raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "weakref finalizer registration failed",
                ),
                crate::builtins::atexit::ExitRegistryError::RegistrationIdExhausted => {
                    raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "atexit registration id space exhausted",
                    )
                }
                crate::builtins::atexit::ExitRegistryError::CapacityExhausted => {
                    raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "weakref finalizer registry capacity exhausted",
                    )
                }
                crate::builtins::atexit::ExitRegistryError::FinalizerGenerationExhausted => {
                    raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "weakref finalizer generation space exhausted",
                    )
                }
                crate::builtins::atexit::ExitRegistryError::InvalidPreparedCapacity => {
                    raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "invalid prepared atexit registry capacity",
                    )
                }
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_finalize_untrack(finalizer_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        crate::builtins::atexit::weakref_finalizer_untrack(_py, finalizer_bits);
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        WeakContainerCookie, builtin_type_id_supports_weakrefs, try_pin_cookie_state,
        weakref_clear_runtime_state,
    };
    use std::sync::atomic::Ordering;

    #[test]
    fn builtin_weakrefability_table_is_explicit_and_fail_closed() {
        for type_id in [
            crate::TYPE_ID_FUNCTION,
            crate::TYPE_ID_MODULE,
            crate::TYPE_ID_TYPE,
            crate::TYPE_ID_GENERATOR,
            crate::TYPE_ID_ASYNC_GENERATOR,
            crate::TYPE_ID_SET,
            crate::TYPE_ID_FROZENSET,
            crate::TYPE_ID_CODE,
        ] {
            assert!(builtin_type_id_supports_weakrefs(type_id));
        }
        for type_id in [
            crate::TYPE_ID_STRING,
            crate::TYPE_ID_LIST,
            crate::TYPE_ID_DICT,
            crate::TYPE_ID_TUPLE,
            crate::TYPE_ID_BYTES,
            crate::TYPE_ID_BYTEARRAY,
            crate::TYPE_ID_RANGE,
            crate::TYPE_ID_SLICE,
            crate::TYPE_ID_EXCEPTION,
            crate::TYPE_ID_PROPERTY,
            crate::TYPE_ID_BOUND_METHOD,
            crate::TYPE_ID_STATICMETHOD,
            crate::TYPE_ID_CLASSMETHOD,
            crate::TYPE_ID_SUPER,
            crate::TYPE_ID_ENUMERATE,
            crate::TYPE_ID_ZIP,
            crate::TYPE_ID_MAP,
            crate::TYPE_ID_FILTER,
            crate::TYPE_ID_ITER,
            crate::TYPE_ID_WEAK_CONTAINER_STATE,
        ] {
            assert!(!builtin_type_id_supports_weakrefs(type_id));
        }
    }

    #[test]
    fn reference_type_cache_releases_its_owned_class_handle() {
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_list(_py, &[]);
            assert!(!ptr.is_null());
            let bits = crate::bits_from_ptr(ptr);
            crate::inc_ref_bits(_py, bits);
            let state = crate::runtime_state(_py);
            let previous = state.weakref_reference_type.swap(bits, Ordering::AcqRel);
            weakref_clear_runtime_state(_py, state);
            assert_eq!(
                unsafe {
                    (*crate::header_from_obj_ptr(ptr))
                        .ref_count
                        .load(Ordering::Acquire)
                },
                1
            );
            if previous != 0 {
                state
                    .weakref_reference_type
                    .store(previous, Ordering::Release);
            }
            crate::dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn cookie_pin_rejects_terminal_state_and_retains_live_state() {
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let state_bits = crate::molt_weakcontainer_new(crate::MoltObject::from_int(1).bits());
        crate::with_gil_entry_nopanic!(_py, {
            let state_ptr = crate::obj_from_bits(state_bits)
                .as_ptr()
                .expect("state ptr");
            let header = unsafe { crate::header_from_obj_ptr(state_ptr) };
            let cookie = WeakContainerCookie {
                state_bits,
                entry: super::WeakEntryId {
                    slot: 0,
                    generation: 1,
                },
            };
            assert!(try_pin_cookie_state(cookie));
            assert_eq!(unsafe { (*header).ref_count.load(Ordering::Acquire) }, 2);
            crate::dec_ref_bits(_py, state_bits);
            unsafe {
                (*header).fetch_or_flags(crate::object::HEADER_FLAG_DEALLOCATING);
            }
            assert!(!try_pin_cookie_state(cookie));
            unsafe {
                (*header).fetch_and_flags(!crate::object::HEADER_FLAG_DEALLOCATING);
            }
            crate::dec_ref_bits(_py, state_bits);
        });
    }

    #[test]
    fn container_cookie_unlinks_for_both_gc_clear_orders() {
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            weakref_clear_runtime_state(_py, crate::runtime_state(_py));
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).object;
            assert!(
                crate::obj_from_bits(crate::molt_weakref_bind_reference_type(reference_type_bits))
                    .is_none()
            );
            let reference_type_ptr = crate::obj_from_bits(reference_type_bits)
                .as_ptr()
                .expect("object class ptr");
            for weakref_first in [true, false] {
                let state_bits =
                    crate::molt_weakcontainer_new(crate::MoltObject::from_int(3).bits());
                let target_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
                let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
                let weak_ptr = crate::obj_from_bits(weak_bits)
                    .as_ptr()
                    .expect("weakref instance ptr");
                assert!(!target_ptr.is_null() && !weak_ptr.is_null());
                let target_bits = crate::bits_from_ptr(target_ptr);
                crate::molt_weakref_register(
                    weak_bits,
                    target_bits,
                    crate::MoltObject::none().bits(),
                );
                crate::molt_weakcontainer_store_commit(
                    state_bits,
                    target_bits,
                    target_bits,
                    weak_bits,
                    crate::MoltObject::from_int(0).bits(),
                );

                if weakref_first {
                    let entry = super::weakref_object_detach(_py, weak_ptr);
                    super::weakref_object_release(_py, entry);
                    assert_eq!(
                        crate::to_i64(crate::obj_from_bits(crate::molt_weakcontainer_len(
                            state_bits,
                        ))),
                        Some(0),
                    );
                    crate::molt_weakcontainer_clear(state_bits);
                } else {
                    crate::molt_weakcontainer_clear(state_bits);
                    let entry = super::weakref_object_detach(_py, weak_ptr);
                    super::weakref_object_release(_py, entry);
                }

                crate::dec_ref_bits(_py, weak_bits);
                crate::dec_ref_bits(_py, target_bits);
                crate::dec_ref_bits(_py, state_bits);
            }
            weakref_clear_runtime_state(_py, crate::runtime_state(_py));
        });
    }
}
