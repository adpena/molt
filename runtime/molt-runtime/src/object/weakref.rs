use super::heap_lifecycle::DetachedEdgeSink;
use super::weak_container::{
    WeakEntryId, weakcontainer_target_dead, weakcontainer_target_dead_detach,
};
use crate::state::runtime_state::{WeakRefEntry, WeakRefRegistry};
use crate::{
    MoltObject, PtrSlot, PyToken, attr_name_bits_from_bytes, molt_get_attr_name, molt_is_callable,
    raise_exception, runtime_state, type_name,
};
use crate::{
    alloc_list, call_callable0, call_callable1, dec_ref_bits, exception_pending,
    header_from_obj_ptr, inc_ref_bits, int_bits_from_i64, is_truthy, obj_from_bits,
};
use std::num::NonZeroU64;
use std::ptr;

pub(crate) const WEAKREF_HASH_UNSET: i64 = -1;

/// Single fail-closed weakrefability authority for runtime registration.
/// Internal/builders and unlisted builtins are rejected even when heap-backed.
pub(crate) fn object_supports_weakrefs(_py: &PyToken<'_>, target_bits: u64) -> bool {
    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
        return false;
    };
    let type_id = unsafe { crate::object_type_id(target_ptr) };
    let policy = crate::object::heap_weakref_policy(type_id)
        .unwrap_or(crate::object::HeapWeakrefPolicy::Deny);
    let class_bits = crate::type_of_bits(_py, target_bits);
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return policy == crate::object::HeapWeakrefPolicy::Allow;
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
    policy == crate::object::HeapWeakrefPolicy::Allow
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WeakContainerCookie {
    state_bits: NonZeroU64,
    pub(crate) entry: WeakEntryId,
}

impl WeakContainerCookie {
    pub(crate) fn new(state_bits: u64, entry: WeakEntryId) -> Option<Self> {
        Some(Self {
            state_bits: NonZeroU64::new(state_bits)?,
            entry,
        })
    }

    #[inline]
    pub(crate) fn state_bits(self) -> u64 {
        self.state_bits.get()
    }
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

fn object_is_weakref_slot(weak_ptr: *mut u8) -> bool {
    (unsafe { crate::object_type_id(weak_ptr) }) == crate::TYPE_ID_WEAKREF
}

/// Upgrade a raw registry link to one ordinary owned reference.
///
/// `registry` is the storage-lifetime token: weakref-object teardown removes
/// `by_ref` membership and referent teardown clears `by_target` membership
/// under the same mutex before either allocation can be freed.  The atomic
/// live retain then closes the remaining rc-to-zero race without resurrecting
/// a terminal object; immortals already are stable owned handles and require no
/// counter mutation.  Callers must verify the pointer's relevant registry
/// membership before invoking this helper and must release the returned bits.
#[inline]
fn try_retain_registered_ptr(_registry: &WeakRefRegistry, registered_ptr: *mut u8) -> Option<u64> {
    if registered_ptr.is_null() {
        return None;
    }
    let header = unsafe { header_from_obj_ptr(registered_ptr) };
    let flags = unsafe { (*header).load_synchronized_flags() };
    if flags & (super::HEADER_FLAG_REVIVAL_WINDOW | super::HEADER_FLAG_DEALLOCATING) != 0 {
        return None;
    }
    let bits = MoltObject::from_ptr(registered_ptr).bits();
    if flags & super::HEADER_FLAG_IMMORTAL != 0 {
        return Some(bits);
    }
    let retained = if flags & super::HEADER_FLAG_HAS_ABI_VIEW != 0 {
        let internal_pins = u32::from(flags & super::HEADER_FLAG_GC_PINNED != 0);
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .transition_runtime_owner_add(bits, false, internal_pins, || unsafe {
                (*header).try_retain_live_previous()
            })
            .is_some()
    } else {
        unsafe { (*header).try_retain_live() }
    };
    retained.then_some(bits)
}

/// Retain a cookie's state object only if its ordinary owner count is still
/// live. Registry custody guarantees state drop cannot detach the cookie and
/// free storage while this inspects its header. The successful path releases
/// registry custody before taking the state lock; state drop likewise releases
/// the state lock before detaching cookies, so the order cannot deadlock.
fn try_pin_cookie_state(cookie: WeakContainerCookie) -> bool {
    let Some(state_ptr) = obj_from_bits(cookie.state_bits()).as_ptr() else {
        return false;
    };
    let header = unsafe { header_from_obj_ptr(state_ptr) };
    unsafe {
        if (*header).type_id != crate::TYPE_ID_WEAK_CONTAINER_STATE
            || ((*header).load_synchronized_flags() & super::HEADER_FLAG_DEALLOCATING) != 0
        {
            return false;
        }
        if (*header).try_retain_live() {
            debug_assert_eq!(
                (*header).load_synchronized_flags() & super::HEADER_FLAG_DEALLOCATING,
                0,
                "state entered terminal death after a successful live retain"
            );
            true
        } else {
            false
        }
    }
}

fn run_pending_weak_death(_py: &PyToken<'_>, death: PendingWeakDeath) {
    if let Some(cookie) = death.cookie {
        // The registry lock pinned state_bits before publishing this work.
        weakcontainer_target_dead(_py, cookie);
        dec_ref_bits(_py, cookie.state_bits());
    }
    if let Some(cb_bits) = death.callback_bits {
        // A registry entry does not own its weakref object.  If that object
        // reached terminal rc while target death held registry custody, its
        // callback edge still transfers here but the callback itself must not
        // run: CPython likewise calls callbacks only for surviving weakrefs.
        if let Some(weak_bits) = death.weak_bits {
            let res_bits = crate::builtins::exceptions::run_unraisable_with_policy(
                _py,
                || weakref_unraisable_policy(_py, cb_bits),
                || unsafe { call_callable1(_py, cb_bits, weak_bits) },
            );
            if !obj_from_bits(res_bits).is_none() {
                dec_ref_bits(_py, res_bits);
            }
            dec_ref_bits(_py, weak_bits);
        }
        dec_ref_bits(_py, cb_bits);
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
            let weak_bits =
                callback_bits.and_then(|_| try_retain_registered_ptr(&registry, weak_slot.0));
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
                // Transfer the registration's owned callback edge into the
                // invocation queue. The non-owning weakref link is upgraded
                // only if that object is still live.
                let weak_bits = try_retain_registered_ptr(&registry, weak_slot.0);
                deaths.push(PendingWeakDeath {
                    weak_bits,
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
    unreachable: &[PtrSlot],
    is_collecting: impl Fn(*mut u8) -> bool,
) {
    let capacity = {
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        unreachable
            .iter()
            .map(|ptr| registry.by_target.get(ptr).map_or(0, Vec::len))
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
        for &PtrSlot(target_ptr) in unreachable {
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
                // Transfer the registration's owned callback edge into the
                // invocation queue. The weakref is a raw registry link.
                let weak_bits = try_retain_registered_ptr(&registry, weak_slot.0);
                deaths.push(PendingWeakDeath {
                    weak_bits,
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
    unreachable: &[PtrSlot],
    is_collecting: &impl Fn(*mut u8) -> bool,
) {
    // Pass 1 clears every target before any callback, preserving CPython's
    // whole-unreachable-set ordering without allocating a side queue.
    {
        let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
        for &PtrSlot(target_ptr) in unreachable {
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
    for &PtrSlot(target_ptr) in unreachable {
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
                    try_retain_registered_ptr(&registry, weak_slot.0)
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
                    dec_ref_bits(_py, cookie.state_bits());
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
    // Remove the raw weakref identity while holding the same custody consumed
    // by `try_retain_registered_ptr`. Terminal object storage cannot be freed
    // until this half of the protocol has completed.
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

pub(crate) fn weakref_object_visit_owned_edges(
    _py: &PyToken<'_>,
    weak_ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    let registry = runtime_state(_py).weakrefs.lock().unwrap();
    let Some(entry) = registry.by_ref.get(&PtrSlot(weak_ptr)) else {
        return;
    };
    visit(entry.callback_bits);
    if let Some(cookie) = entry.container_cookie {
        visit(cookie.state_bits());
    }
}

pub(crate) fn weakref_object_detach_owned_edges(
    _py: &PyToken<'_>,
    weak_ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    let Some(entry) = unregister_weakref(_py, weak_ptr) else {
        return;
    };
    if let Some(cookie) = entry.container_cookie {
        weakcontainer_target_dead_detach(_py, cookie, sink);
        sink.detach_if_heap(cookie.state_bits());
    }
    sink.detach_if_heap(entry.callback_bits);
}

pub(crate) fn weakref_object_terminal_extra_edge_count(
    _py: &PyToken<'_>,
    weak_ptr: *mut u8,
) -> usize {
    let cookie = {
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        registry
            .by_ref
            .get(&PtrSlot(weak_ptr))
            .and_then(|entry| entry.container_cookie)
    };
    cookie.map_or(
        0,
        super::weak_container::weakcontainer_target_dead_detach_edge_count,
    )
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
    try_retain_registered_ptr(&registry, target_ptr)
}

fn weakref_get_owned_or_none(_py: &PyToken<'_>, weak_bits: u64) -> u64 {
    if obj_from_bits(weak_bits).as_ptr().is_none() {
        return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
    }
    weakref_peek_owned(_py, weak_bits).unwrap_or_else(|| MoltObject::none().bits())
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
            if let Some(weak_bits) = try_retain_registered_ptr(&registry, weak_ptr) {
                out.push(weak_bits);
            }
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
                if weak_slot.0.is_null()
                    || unsafe { crate::object_type_id(weak_slot.0) } != crate::TYPE_ID_WEAKREF
                    || unsafe { crate::object_class_bits(weak_slot.0) }
                        != crate::builtins::classes::builtin_classes(_py).reference_type
                {
                    continue;
                }
                let Some(entry) = registry.by_ref.get(weak_slot) else {
                    continue;
                };
                if entry.target == target_slot && obj_from_bits(entry.callback_bits).is_none() {
                    if let Some(weak_bits) = try_retain_registered_ptr(&registry, weak_slot.0) {
                        return weak_bits;
                    }
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

/// Return the first registered weak reference to `target_bits` as an owned
/// handle, or None when no live reference exists. This is the managed
/// `__weakref__` descriptor primitive; it deliberately avoids materializing
/// the public `getweakrefs()` list.
pub(crate) fn weakref_head_for_target(_py: &PyToken<'_>, target_bits: u64) -> u64 {
    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    let registry = runtime_state(_py).weakrefs.lock().unwrap();
    let target_slot = PtrSlot(target_ptr);
    let Some(ref_slots) = registry.by_target.get(&target_slot) else {
        return MoltObject::none().bits();
    };
    for weak_slot in ref_slots {
        if weak_slot.0.is_null() {
            continue;
        }
        let Some(entry) = registry.by_ref.get(weak_slot) else {
            continue;
        };
        if entry.target == target_slot && !entry.target.0.is_null() {
            if let Some(bits) = try_retain_registered_ptr(&registry, weak_slot.0) {
                return bits;
            }
        }
    }
    MoltObject::none().bits()
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
        if !object_is_weakref_slot(weak_ptr) {
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
        if unsafe { (*header_from_obj_ptr(target_ptr)).load_synchronized_flags() }
            & super::HEADER_FLAG_DEALLOCATING
            != 0
        {
            return raise_exception::<_>(
                _py,
                "ReferenceError",
                "cannot create weak reference to deallocating object",
            );
        }
        if crate::object::ops_sys::runtime_target_minor(_py) >= 15
            && !obj_from_bits(callback_bits).is_none()
        {
            let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(callback_bits)));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !callable_ok {
                let callback_type = type_name(_py, obj_from_bits(callback_bits));
                let message = format!("callback must be callable or None, not '{callback_type}'");
                return raise_exception::<_>(_py, "TypeError", &message);
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
                        cached_hash: WEAKREF_HASH_UNSET,
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

/// Return a weakref's sticky CPython hash, computing it from a live referent at
/// most once.  The registry lock is never held across user `__hash__` code.
/// A successfully pinned referent cannot die during computation; after the
/// call, the result is committed only if this registration still names the
/// same referent.  Cached lookup is callback-free and remains valid after
/// referent death.
pub(crate) fn weakref_cached_hash_or_compute(_py: &PyToken<'_>, weak_bits: u64) -> i64 {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return raise_exception::<i64>(_py, "TypeError", "weakref must be an object");
    };
    if !object_is_weakref_slot(weak_ptr) {
        return raise_exception::<i64>(_py, "TypeError", "weakref must be an object");
    }
    let weak_slot = PtrSlot(weak_ptr);
    let target_bits = {
        let registry = runtime_state(_py).weakrefs.lock().unwrap();
        let Some(entry) = registry.by_ref.get(&weak_slot) else {
            return raise_exception::<i64>(_py, "TypeError", "weak object has gone away");
        };
        if entry.cached_hash != WEAKREF_HASH_UNSET {
            return entry.cached_hash;
        }
        if entry.target.0.is_null() {
            return raise_exception::<i64>(_py, "TypeError", "weak object has gone away");
        }
        let Some(bits) = try_retain_registered_ptr(&registry, entry.target.0) else {
            return raise_exception::<i64>(_py, "TypeError", "weak object has gone away");
        };
        bits
    };

    let target_ptr = obj_from_bits(target_bits)
        .as_ptr()
        .unwrap_or_else(|| std::process::abort());
    let hash = super::ops_hash::hash_bits_signed(_py, target_bits);
    dec_ref_bits(_py, target_bits);
    if exception_pending(_py) {
        return 0;
    }

    let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
    if let Some(entry) = registry.by_ref.get_mut(&weak_slot) {
        if entry.cached_hash == WEAKREF_HASH_UNSET
            && (entry.target.0.is_null() || entry.target.0 == target_ptr)
        {
            entry.cached_hash = hash;
        }
        return if entry.cached_hash == WEAKREF_HASH_UNSET {
            hash
        } else {
            entry.cached_hash
        };
    }
    hash
}

/// Publish a hash already computed by a weak container into the registry's
/// canonical sticky-hash slot. WeakKeyDictionary and WeakSet index their
/// tables with this value, so calling user `__hash__` a second time would be
/// redundant and could disagree with the table bucket.
pub(crate) fn weakref_seed_cached_hash(
    _py: &PyToken<'_>,
    weak_bits: u64,
    expected_target_bits: u64,
    hash: i64,
) -> bool {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return false;
    };
    if !object_is_weakref_slot(weak_ptr) {
        return false;
    }
    let mut registry = runtime_state(_py).weakrefs.lock().unwrap();
    let Some(entry) = registry.by_ref.get_mut(&PtrSlot(weak_ptr)) else {
        return false;
    };
    if entry.target.0.is_null()
        || MoltObject::from_ptr(entry.target.0).bits() != expected_target_bits
        || hash == WEAKREF_HASH_UNSET
    {
        return false;
    }
    if entry.cached_hash == WEAKREF_HASH_UNSET {
        entry.cached_hash = hash;
    }
    entry.cached_hash == hash
}

pub(crate) fn weakref_has_live_target(
    _py: &PyToken<'_>,
    weak_bits: u64,
    expected_target_bits: u64,
) -> bool {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return false;
    };
    if !object_is_weakref_slot(weak_ptr) {
        return false;
    }
    runtime_state(_py)
        .weakrefs
        .lock()
        .unwrap()
        .by_ref
        .get(&PtrSlot(weak_ptr))
        .is_some_and(|entry| {
            !entry.target.0.is_null()
                && MoltObject::from_ptr(entry.target.0).bits() == expected_target_bits
        })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_get(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { weakref_get_owned_or_none(_py, weak_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_callback(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { weakref_callback_owned(_py, weak_bits) })
}

pub(crate) fn weakref_callback_owned(_py: &PyToken<'_>, weak_bits: u64) -> u64 {
    let Some(weak_ptr) = obj_from_bits(weak_bits).as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "weakref must be an object");
    };
    let weak_slot = PtrSlot(weak_ptr);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_peek(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { weakref_get_owned_or_none(_py, weak_bits) })
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
        WeakContainerCookie, try_pin_cookie_state, weakref_cached_hash_or_compute,
        weakref_head_for_target, weakref_peek_owned, weakref_seed_cached_hash,
        weakref_snapshot_for_target,
    };

    #[test]
    fn weakref_registry_entry_stays_cache_compact() {
        assert_eq!(
            std::mem::size_of::<Option<WeakContainerCookie>>(),
            std::mem::size_of::<WeakContainerCookie>(),
            "the nonzero state handle must provide the Option niche",
        );
        assert!(
            std::mem::size_of::<crate::state::runtime_state::WeakRefEntry>() <= 48,
            "every registered weakref should leave headroom within one cache line",
        );
    }

    #[test]
    fn seeded_hash_is_idempotent_and_survives_target_death() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).reference_type;
            let reference_type_ptr = crate::obj_from_bits(reference_type_bits)
                .as_ptr()
                .expect("reference type ptr");
            let target_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let target_bits = crate::bits_from_ptr(target_ptr);
            let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
            assert_eq!(
                crate::molt_weakref_register(
                    weak_bits,
                    target_bits,
                    crate::MoltObject::none().bits(),
                ),
                crate::MoltObject::from_bool(true).bits(),
            );
            assert!(weakref_seed_cached_hash(_py, weak_bits, target_bits, 313));
            assert!(weakref_seed_cached_hash(_py, weak_bits, target_bits, 313));
            assert!(!weakref_seed_cached_hash(_py, weak_bits, target_bits, 919));
            assert!(!weakref_seed_cached_hash(
                _py,
                weak_bits,
                crate::MoltObject::none().bits(),
                313,
            ));
            crate::dec_ref_bits(_py, target_bits);
            assert_eq!(weakref_cached_hash_or_compute(_py, weak_bits), 313);
            crate::dec_ref_bits(_py, weak_bits);
        });
    }

    #[test]
    fn registration_does_not_retain_the_weakref_object() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).reference_type;
            let reference_type_ptr = crate::obj_from_bits(reference_type_bits)
                .as_ptr()
                .expect("reference type ptr");
            let target_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let target_bits = crate::bits_from_ptr(target_ptr);
            let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
            let weak_ptr = crate::obj_from_bits(weak_bits)
                .as_ptr()
                .expect("weakref ptr");
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(weak_ptr)).ref_count_snapshot() },
                1
            );
            assert_eq!(
                crate::molt_weakref_register(
                    weak_bits,
                    target_bits,
                    crate::MoltObject::none().bits(),
                ),
                crate::MoltObject::from_bool(true).bits(),
            );
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(weak_ptr)).ref_count_snapshot() },
                1
            );
            crate::dec_ref_bits(_py, weak_bits);
            assert!(
                !crate::runtime_state(_py)
                    .weakrefs
                    .lock()
                    .unwrap()
                    .by_ref
                    .contains_key(&crate::PtrSlot(weak_ptr))
            );
            crate::dec_ref_bits(_py, target_bits);
        });
    }

    #[test]
    fn registry_lookup_family_retains_live_edges_once_and_rejects_terminal_objects() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).reference_type;
            let reference_type_ptr = crate::obj_from_bits(reference_type_bits)
                .as_ptr()
                .expect("reference type ptr");
            let target_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let target_bits = crate::bits_from_ptr(target_ptr);
            let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
            let weak_ptr = crate::obj_from_bits(weak_bits)
                .as_ptr()
                .expect("weakref ptr");
            let target_header = unsafe { crate::header_from_obj_ptr(target_ptr) };
            let weak_header = unsafe { crate::header_from_obj_ptr(weak_ptr) };
            assert_eq!(
                crate::molt_weakref_register(
                    weak_bits,
                    target_bits,
                    crate::MoltObject::none().bits(),
                ),
                crate::MoltObject::from_bool(true).bits(),
            );

            let owned_target = weakref_peek_owned(_py, weak_bits).expect("live target pin");
            assert_eq!(owned_target, target_bits);
            assert_eq!(unsafe { (*target_header).ref_count_snapshot() }, 2);
            crate::dec_ref_bits(_py, owned_target);

            for lookup in [
                crate::molt_weakref_get as extern "C" fn(u64) -> u64,
                crate::molt_weakref_peek as extern "C" fn(u64) -> u64,
            ] {
                let owned_target = lookup(weak_bits);
                assert_eq!(owned_target, target_bits);
                assert_eq!(unsafe { (*target_header).ref_count_snapshot() }, 2);
                crate::dec_ref_bits(_py, owned_target);
            }

            let snapshot = weakref_snapshot_for_target(_py, target_ptr).expect("weakref snapshot");
            assert_eq!(snapshot, [weak_bits]);
            assert_eq!(unsafe { (*weak_header).ref_count_snapshot() }, 2);
            crate::dec_ref_bits(_py, snapshot[0]);

            let cached = crate::molt_weakref_find_nocallback(target_bits);
            assert_eq!(cached, weak_bits);
            assert_eq!(unsafe { (*weak_header).ref_count_snapshot() }, 2);
            crate::dec_ref_bits(_py, cached);

            let head = weakref_head_for_target(_py, target_bits);
            assert_eq!(head, weak_bits);
            assert_eq!(unsafe { (*weak_header).ref_count_snapshot() }, 2);
            crate::dec_ref_bits(_py, head);

            unsafe {
                (*target_header).fetch_or_flags(crate::object::HEADER_FLAG_REVIVAL_WINDOW);
            }
            assert_eq!(weakref_peek_owned(_py, weak_bits), None);
            unsafe {
                (*target_header).fetch_and_flags(!crate::object::HEADER_FLAG_REVIVAL_WINDOW);
                (*target_header).fetch_or_flags(crate::object::HEADER_FLAG_DEALLOCATING);
            }
            assert_eq!(weakref_peek_owned(_py, weak_bits), None);
            assert_eq!(
                crate::molt_weakref_get(weak_bits),
                crate::MoltObject::none().bits(),
            );
            assert_eq!(
                crate::molt_weakref_peek(weak_bits),
                crate::MoltObject::none().bits(),
            );
            unsafe {
                (*target_header).fetch_and_flags(!crate::object::HEADER_FLAG_DEALLOCATING);
                (*weak_header).fetch_or_flags(crate::object::HEADER_FLAG_DEALLOCATING);
            }
            assert!(
                weakref_snapshot_for_target(_py, target_ptr)
                    .expect("terminal weakref snapshot")
                    .is_empty()
            );
            assert_eq!(
                crate::molt_weakref_find_nocallback(target_bits),
                crate::MoltObject::none().bits(),
            );
            assert_eq!(
                weakref_head_for_target(_py, target_bits),
                crate::MoltObject::none().bits(),
            );
            unsafe {
                (*weak_header).fetch_and_flags(!crate::object::HEADER_FLAG_DEALLOCATING);
            }

            assert_eq!(unsafe { (*target_header).ref_count_snapshot() }, 1);
            assert_eq!(unsafe { (*weak_header).ref_count_snapshot() }, 1);
            crate::dec_ref_bits(_py, weak_bits);
            crate::dec_ref_bits(_py, target_bits);
        });
    }

    #[test]
    fn registry_live_retain_preserves_immortal_referent_semantics() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).reference_type;
            let reference_type_ptr = crate::obj_from_bits(reference_type_bits)
                .as_ptr()
                .expect("reference type ptr");
            let target_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let target_bits = crate::bits_from_ptr(target_ptr);
            let target_header = unsafe { crate::header_from_obj_ptr(target_ptr) };
            unsafe {
                (*target_header).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
                (*target_header).make_immortal();
            }
            assert_ne!(
                unsafe { (*target_header).load_synchronized_flags() }
                    & crate::object::HEADER_FLAG_IMMORTAL,
                0,
            );
            let before = unsafe { (*target_header).ref_count_snapshot() };
            let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
            assert_eq!(
                crate::molt_weakref_register(
                    weak_bits,
                    target_bits,
                    crate::MoltObject::none().bits(),
                ),
                crate::MoltObject::from_bool(true).bits(),
            );
            let owned_target = weakref_peek_owned(_py, weak_bits).expect("immortal target pin");
            assert_eq!(owned_target, target_bits);
            assert_eq!(unsafe { (*target_header).ref_count_snapshot() }, before);
            crate::dec_ref_bits(_py, owned_target);
            crate::dec_ref_bits(_py, weak_bits);
            crate::object::release_shutdown_owned_bits(_py, target_bits);
        });
    }

    #[test]
    fn terminal_weakref_suppresses_callback_and_releases_transferred_edge() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let callback_ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
            let callback_bits = crate::bits_from_ptr(callback_ptr);
            let callback_header = unsafe { crate::header_from_obj_ptr(callback_ptr) };
            crate::inc_ref_bits(_py, callback_bits);
            assert_eq!(unsafe { (*callback_header).ref_count_snapshot() }, 2);
            super::run_pending_weak_death(
                _py,
                super::PendingWeakDeath {
                    weak_bits: None,
                    callback_bits: Some(callback_bits),
                    cookie: None,
                },
            );
            assert!(!crate::exception_pending(_py));
            assert_eq!(unsafe { (*callback_header).ref_count_snapshot() }, 1);
            crate::dec_ref_bits(_py, callback_bits);
        });
    }

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
            crate::TYPE_ID_MEMORYVIEW,
            crate::TYPE_ID_GENERIC_ALIAS,
            crate::TYPE_ID_BOUND_METHOD,
        ] {
            assert_eq!(
                crate::object::heap_weakref_policy(type_id),
                Some(crate::object::HeapWeakrefPolicy::Allow)
            );
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
            assert_eq!(
                crate::object::heap_weakref_policy(type_id),
                Some(crate::object::HeapWeakrefPolicy::Deny)
            );
        }
    }

    #[test]
    fn cookie_pin_rejects_terminal_state_and_retains_live_state() {
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        let state_bits = crate::molt_weakcontainer_new(crate::MoltObject::from_int(1).bits());
        crate::with_gil_entry_nopanic!(_py, {
            let state_ptr = crate::obj_from_bits(state_bits)
                .as_ptr()
                .expect("state ptr");
            let header = unsafe { crate::header_from_obj_ptr(state_ptr) };
            let cookie = WeakContainerCookie::new(
                state_bits,
                super::WeakEntryId {
                    slot: 0,
                    generation: 1,
                },
            )
            .expect("heap state bits are nonzero");
            assert!(try_pin_cookie_state(cookie));
            assert_eq!(unsafe { (*header).ref_count_snapshot() }, 2);
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
        let _lock = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let reference_type_bits = crate::builtins::classes::builtin_classes(_py).reference_type;
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
                assert_eq!(
                    unsafe { crate::object_type_id(weak_ptr) },
                    crate::TYPE_ID_WEAKREF
                );
                crate::molt_weakcontainer_store_commit(
                    state_bits,
                    target_bits,
                    target_bits,
                    weak_bits,
                    crate::MoltObject::from_int(0).bits(),
                );

                if weakref_first {
                    let mut sink = super::DetachedEdgeSink::try_with_capacities(3, 0)
                        .expect("weakref test sink allocation");
                    super::weakref_object_detach_owned_edges(_py, weak_ptr, &mut sink);
                    sink.release_all(_py);
                    assert_eq!(
                        crate::to_i64(crate::obj_from_bits(crate::molt_weakcontainer_len(
                            state_bits,
                        ))),
                        Some(0),
                    );
                    crate::molt_weakcontainer_clear(state_bits);
                } else {
                    crate::molt_weakcontainer_clear(state_bits);
                    let mut sink = super::DetachedEdgeSink::try_with_capacities(3, 0)
                        .expect("weakref test sink allocation");
                    super::weakref_object_detach_owned_edges(_py, weak_ptr, &mut sink);
                    sink.release_all(_py);
                }

                crate::dec_ref_bits(_py, weak_bits);
                crate::dec_ref_bits(_py, target_bits);
                crate::dec_ref_bits(_py, state_bits);
            }
        });
    }
}
